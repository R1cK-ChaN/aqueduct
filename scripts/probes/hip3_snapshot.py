import json, urllib.request, time
def post(body):
    req = urllib.request.Request('https://api.hyperliquid.xyz/info', data=json.dumps(body).encode(), headers={'Content-Type':'application/json'})
    with urllib.request.urlopen(req, timeout=20) as r:
        return json.load(r)
dexs = post({'type':'perpDexs'})
print('dex names', [d['name'] for d in dexs if d])
for d in dexs:
    if d: print(d['name'], {k:(v if not isinstance(v,list) else f'list[{len(v)}]') for k,v in d.items()})
allrows=[]
for d in dexs:
    if not d: continue
    try:
        meta, ctxs = post({'type':'metaAndAssetCtxs','dex':d['name']})
    except Exception as e:
        print('err', d['name'], e); continue
    tot=0
    for u,c in zip(meta['universe'],ctxs):
        if u.get('isDelisted'): continue
        v=float(c['dayNtlVlm']); tot+=v
        allrows.append((v,u['name'],u))
    print(d['name'], 'n', len(meta['universe']), 'vlm$M', round(tot/1e6,1), 'meta keys', [k for k in meta.keys()], 'collateralToken', meta.get('collateralToken'))
allrows.sort(key=lambda r:-r[0])
for v,n,u in allrows[:25]: print(n, round(v/1e6,1), u)
json.dump([(n,v,u) for v,n,u in allrows], open('hip3_rank.json','w'))
print(json.dumps(post({'type':'userFees','user':'0x0000000000000000000000000000000000000001'}))[:1500])
print(json.dumps(post({'type':'userRateLimit','user':'0x0000000000000000000000000000000000000001'})))
