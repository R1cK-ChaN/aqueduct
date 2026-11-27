# Aqueduct

[English](README.md) · [简体中文](README.zh-CN.md)

[![CI](https://github.com/R1cK-ChaN/aqueduct/actions/workflows/ci.yml/badge.svg)](https://github.com/R1cK-ChaN/aqueduct/actions/workflows/ci.yml)
[![Licence: MIT OR Apache-2.0](https://img.shields.io/badge/licence-MIT%20OR%20Apache--2.0-blue.svg)](#许可证)

Aqueduct 是给订单簿 DEX 做市用的一套基础工具：录行情，把录下来的行情放到模拟交易所上回放，再用同一份策略代码接实时行情跑模拟盘。目前支持 Hyperliquid 永续合约。

## 为什么开源

做市的人多了，价差会变小，来交易的人都能拿到更好的价格。可要开始做市，先得有行情录制、一套跑两次结果相同的回放，还有模拟盘。这些事工作量不小，和你的策略却没多大关系。我们把它做完并公开，你或者你的 Agent 可以直接从写策略开始。名字也是这个意思：aqueduct 是引水渠，这里的水是流动性。

## 跑一下

需要 Linux、通过 [rustup](https://rustup.rs) 安装的 Rust（工具链版本已固定）和 `zstd`。

仓库里有一段 60 秒的单一市场录制，是测试用的样本。下面三条命令先压缩它，再用示例报价策略跑一遍，最后核对这次运行写出的会话文件。

```bash
zstd -q -o /tmp/sample.hlrec.zst crates/hl-codec/tests/fixtures/hl_ws_pons_20261124.hlrec
```

```bash
cargo run --release -p hl-backtest -- run \
  --coin PONS --half-spread 0.00002 --qty 500 \
  --order-latency-ms 400 --report-latency-ms 250 --maker-fee-bps 1.5 \
  --session-out /tmp/session.jsonl /tmp/sample.hlrec.zst
```

```bash
cargo run --release -p hl-backtest -- run --check-session /tmp/session.jsonl
```

第二条命令打印这次运行的 JSON 摘要，第三条打印 `session matches` 和行数。这只说明整条流程能跑通。60 秒的单一市场说明不了策略的好坏。前两条命令不会覆盖已有文件，重跑之前先删掉 `/tmp/sample.hlrec.zst` 和 `/tmp/session.jsonl`。

每个参数和输出字段的含义、怎么录自己的数据、怎么跑模拟盘，见 [docs/usage.md](docs/usage.md)（英文）。

## 整条流程

| 步骤 | 程序 | 输入 | 输出 |
| --- | --- | --- | --- |
| 录制 | `hl-recorder` | 公开行情 | 原始录制文件 |
| 入库和检查 | `hl-loader` | 录制文件和官方归档 | ClickHouse 表、每日数据质量检查 |
| 回测 | `hl-backtest run` | 录制文件和你的策略 | 摘要和会话文件 |
| 模拟盘 | `hl-live paper` | 实时行情和你的策略 | 它自己的录制文件和会话文件 |
| 核对 | `hl-backtest run --check-session` | 会话文件和对应的录制文件 | 一致，或第一个不一致的字段 |

回测和模拟盘跑的是同一份策略代码。

## 这里没有什么

- 策略。只有一个示例 `FixedSpreadQuoter`，为的是让流程有东西可跑。
- 研究方法。跑哪些组合、怎么评判结果，由你决定。
- 任何会向交易所下单的代码。下单网关是一个接口，现在接在它后面的是模拟器。
- 结果。

模拟器是一个模型。它做了哪些假设、每条假设要在实盘账户上测什么才能验证，列在 [RFC 0008 D8](docs/rfc/0008-paper-trading.md#d8-replay-the-recording-and-compare-decisions-in-order) 的 “Limits against live trading” 一段。

## 现状

| 部分 | Crate | 状态 |
| --- | --- | --- |
| 公开行情录制器 | `hl-recorder` | 可用。[HL-41](docs/venues/hyperliquid.md#hl-41)、[BN-8](docs/venues/binance.md#bn-8) |
| 解码器和 L2 订单簿 | `hl-codec`、`hl-book` | 支持 Hyperliquid 的行情帧和官方归档，订单簿可由增量重建。[HL-97](docs/venues/hyperliquid.md#hl-97)、[HL-105](docs/venues/hyperliquid.md#hl-105) |
| 入库和每日数据质量检查 | `hl-loader` | 可用 |
| 模拟交易所 | `hl-sim` | 可用，功能最小：只接受被动挂单，成交价严格穿过挂单价才算成交 |
| 回放和回测 | `hl-run`、`hl-backtest` | 可用，每次运行一个币种 |
| 模拟盘 | `hl-live` | 可用 |
| 按日期生效的价格和数量规则 | `hl-rules` | 已写好，模拟器还没用上。[RFC 0009 D7](docs/rfc/0009-instrument-identity.md#d7-supply-dated-rules-to-backtest-and-paper-runs) |
| 公开的录制数据集 | `hl-publish` | 已设计，还没有提供。[RFC 0010 D8](docs/rfc/0010-public-recording-dataset.md#d8-order-of-later-issues) |
| 实盘下单网关 | `hl-gateway` | 只有接口 |
| 风控和看门狗 | `hl-risk`、`hl-watchdog` | 还没开始 |

## 代码遵守的几条规则

- 同样的输入得到逐字节相同的输出。回测结果依赖的地方不读系统时钟，也不用没有种子的随机数。
- 会话文件可以重放并逐字段比较，命令是 `hl-backtest run --check-session`。
- 策略不做任何读写，只接收事件、返回动作。
- 凡是说到交易所怎么运作，都引用登记册里的条目，条目带来源和核对日期。

有了这几条，Agent 和人一样能在这里干活：它可以自己检查结果对不对。两者共用的工作规则在 [AGENTS.md](AGENTS.md)。

## 文档

- [使用说明](docs/usage.md)：构建、测试、运行回测和模拟盘。
- [文档地图](docs/INDEX.md)：每份文档和它负责的内容。
- [架构](docs/ARCHITECTURE.md)：crate、依赖关系和数据流向。
- [RFC 0001](docs/rfc/0001-core-types-and-interfaces.md)：核心接口为什么是现在的样子。
- [交易所登记册](docs/venues/README.md)：交易所规则和测量结果，每条都有来源。

本页之外的文档目前只有英文。

## 路线图

- 下一个交易所是 Lighter。crate 名带 `hl-`，是因为当初只为一个交易所而写。接第二个的时候会改名。
- 让模拟器用上按日期生效的价格和数量规则。
- 公开的录制数据集。
- `learn/` 目录：做市者在做什么，回测会在哪些地方误导人。

## 参与贡献

先读 [AGENTS.md](AGENTS.md)。改动共享接口，先在 `docs/rfc/` 写 RFC。 Issue 和 PR 用英文。

## 许可证

你可以任选 MIT 许可证或 Apache 许可证 2.0 版使用本项目的代码（`MIT OR Apache-2.0`）。[MIT](LICENSE-MIT) · [Apache-2.0](LICENSE-APACHE)

除非你明确另行声明，你有意提交并入本项目的任何贡献（定义见 Apache 许可证 2.0 版）都按上述双许可授权，不附加其他条款或条件。
