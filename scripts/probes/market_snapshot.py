import json, urllib.request, time
def post(body):
    req = urllib.request.Request('https://api.hyperliquid.xyz/info', data=json.dumps(body).encode(), headers={'Content-Type':'application/json'})
    t=time.time()
    with urllib.request.urlopen(req, timeout=20) as r:
        d=json.load(r)
    return d, (time.time()-t)*1000
(meta, ctxs), ms = post({'type':'metaAndAssetCtxs'})
print('rest latency ms', round(ms))
print('n perps main dex', len(meta['universe']))
print('sample universe entry', meta['universe'][0])
print('sample ctx', ctxs[0])
rows=[]
for u,c in zip(meta['universe'],ctxs):
    if u.get('isDelisted'): continue
    rows.append((float(c['dayNtlVlm']), u['name'], u['szDecimals'], u['maxLeverage'], c))
rows.sort(reverse=True)
print('total 24h ntl vlm main dex $M', round(sum(r[0] for r in rows)/1e6))
json.dump([(r[1],r[0],r[2],r[3]) for r in rows], open('perps_rank.json','w'))
for r in rows[:60]:
    print(r[1], 'vlm$M', round(r[0]/1e6,1), 'szDec', r[2], 'maxLev', r[3], 'mark', r[4]['markPx'], 'fund', r[4]['funding'], 'OI$M', round(float(r[4]['openInterest'])*float(r[4]['markPx'])/1e6,1))
dexs,_ = post({'type':'perpDexs'})
print('perpDexs:', json.dumps(dexs)[:3000])
