"""Shared disposable ClickHouse harness for the loader's opt-in CLI tests."""

import base64
import json
import os
from pathlib import Path
import re
import secrets
import socket
import subprocess
import tempfile
import time
import unittest
import urllib.error
import urllib.request
import uuid


ROOT = Path(__file__).resolve().parents[3]
LOADER = ROOT / "target/debug/hl-loader"
DOCKER = ("docker", "--host", "unix:///var/run/docker.sock")


def free_port():
    while True:
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        if port not in (8123, 18123):
            return port


@unittest.skipUnless(
    os.environ.get("HLMM_TEST_CLICKHOUSE") == "1",
    "set HLMM_TEST_CLICKHOUSE=1 to start an isolated test container",
)
class DisposableClickHouseTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if not LOADER.is_file():
            raise AssertionError("build target/debug/hl-loader with cargo build -p hl-loader")
        cls.tmp = tempfile.TemporaryDirectory(prefix="hlmm-clickhouse-test-")
        cls.addClassCleanup(cls.tmp.cleanup)
        cls.directory = Path(cls.tmp.name)
        cls.cidfile = cls.directory / "container.id"
        cls.container_name = f"hlmm-test-{uuid.uuid4().hex}"
        cls.environment = {"PATH": os.environ.get("PATH", os.defpath)}
        cls.addClassCleanup(cls.remove_container)
        cls.port = free_port()
        cls.url = f"http://127.0.0.1:{cls.port}"
        cls.http = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        cls.username = "loader_test"
        cls.password = secrets.token_urlsafe(32)
        cls.authorization = "Basic " + base64.b64encode(
            f"{cls.username}:{cls.password}".encode()
        ).decode()
        created = subprocess.run(
            [
                *DOCKER,
                "create",
                "--pull=never",
                "--name",
                cls.container_name,
                "--cidfile",
                str(cls.cidfile),
                "--publish",
                f"127.0.0.1:{cls.port}:8123",
                "--tmpfs",
                "/var/lib/clickhouse:rw",
                "--tmpfs",
                "/var/log/clickhouse-server:rw",
                "--env",
                "CLICKHOUSE_USER",
                "--env",
                "CLICKHOUSE_PASSWORD",
                "clickhouse/clickhouse-server:26.8",
            ],
            env={
                **cls.environment,
                "CLICKHOUSE_USER": cls.username,
                "CLICKHOUSE_PASSWORD": cls.password,
            },
            capture_output=True,
            text=True,
            timeout=30,
            check=True,
        )
        cls.container_id = cls.cidfile.read_text().strip()
        if not re.fullmatch(r"[0-9a-f]{64}", cls.container_id):
            raise AssertionError("Docker did not write a full container ID")
        if created.stdout.strip() != cls.container_id:
            raise AssertionError("Docker output and container ID file disagree")
        subprocess.run(
            [*DOCKER, "start", cls.container_id],
            env=cls.environment,
            capture_output=True,
            text=True,
            timeout=30,
            check=True,
        )
        deadline = time.monotonic() + 45
        while True:
            try:
                if cls.sql("SELECT 1").strip() == "1":
                    break
            except (OSError, urllib.error.URLError, AssertionError):
                pass
            if time.monotonic() >= deadline:
                raise AssertionError("the disposable ClickHouse did not become ready")
            time.sleep(0.2)
        print(
            f"Disposable ClickHouse {cls.sql('SELECT version()').strip()} "
            f"on 127.0.0.1:{cls.port}, container {cls.container_id}"
        )
        cls.first_migration = cls.cli("migrate")[0]

    @classmethod
    def remove_container(cls):
        if cls.cidfile.exists():
            container_id = cls.cidfile.read_text().strip()
        else:
            # Creation can fail after Docker reserves the unique test name.
            found = subprocess.run(
                [*DOCKER, "inspect", "--format", "{{.Id}}", cls.container_name],
                env=cls.environment,
                capture_output=True,
                text=True,
                timeout=10,
                check=False,
            )
            if found.returncode != 0:
                diagnostic = found.stdout + found.stderr
                if "no such" in diagnostic.lower() and cls.container_name in diagnostic:
                    return
                raise AssertionError(f"could not inspect test-container cleanup target\n{diagnostic}")
            container_id = found.stdout.strip()
        if not re.fullmatch(r"[0-9a-f]{64}", container_id):
            raise AssertionError("refusing cleanup without a valid captured container ID")
        subprocess.run(
            [*DOCKER, "rm", "--force", "--volumes", container_id],
            env=cls.environment,
            capture_output=True,
            text=True,
            timeout=30,
            check=True,
        )
        absent = subprocess.run(
            [*DOCKER, "inspect", "--format", "{{.Id}}", container_id],
            env=cls.environment,
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )
        diagnostic = absent.stdout + absent.stderr
        if (
            absent.returncode == 0
            or "no such" not in diagnostic.lower()
            or container_id not in diagnostic
        ):
            raise AssertionError(f"could not verify test-container removal\n{diagnostic}")
        print(f"Removed disposable container {container_id[:12]} and its volumes")

    @classmethod
    def sql(cls, query):
        request = urllib.request.Request(cls.url, data=query.encode())
        request.add_header("Authorization", cls.authorization)
        try:
            with cls.http.open(request, timeout=3) as response:
                return response.read().decode()
        except urllib.error.HTTPError as error:
            raise AssertionError(error.read().decode()) from error

    @classmethod
    def rows(cls, query):
        return [json.loads(line) for line in cls.sql(query + " FORMAT JSONEachRow").splitlines()]

    @classmethod
    def run_cli(cls, *arguments, expected_code=0):
        environment = {
            **cls.environment,
            "HLMM_CLICKHOUSE_URL": cls.url,
            "HLMM_CLICKHOUSE_USER": cls.username,
            "HLMM_CLICKHOUSE_PASSWORD": cls.password,
        }
        result = subprocess.run(
            [str(LOADER), *(str(argument) for argument in arguments)],
            env=environment,
            capture_output=True,
            text=True,
            timeout=30,
            check=False,
        )
        if result.returncode != expected_code:
            raise AssertionError(
                f"loader exited {result.returncode}, expected {expected_code}\n"
                f"{result.stdout}{result.stderr}"
            )
        return result

    @classmethod
    def cli(cls, *arguments, expected_code=0):
        result = cls.run_cli(*arguments, expected_code=expected_code)
        return [json.loads(line) for line in result.stdout.splitlines()]
