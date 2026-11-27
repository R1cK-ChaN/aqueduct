import websocket, json, time, statistics, collections
ws = websocket.create_connection("wss://api.hyperliquid.xyz/ws", timeout=10)
subs = [{"type":"l2Book","coin":"BTC"},{"type":"bbo","coin":"BTC"},{"type":"trades","coin":"BTC"},{"type":"l2","c":"BTC"}]
for s in subs:
    ws.send(json.dumps({"method":"subscribe","subscription":s}))
t0=time.time(); arr=collections.defaultdict(list); lv={}; lag=collections.defaultdict(list)
while time.time()-t0 < 30:
    try: m=json.loads(ws.recv())
    except Exception as e: print('err',e); break
    ch=m.get('channel'); now=time.time()
    if ch in ('l2Book','bbo','trades','l2'):
        arr[ch].append(now)
        d=m.get('data')
        if ch=='l2Book': lv[ch]=len(d['levels'][0]); lag[ch].append(now*1000-d['time'])
        if ch=='bbo': lag[ch].append(now*1000-d['time'])
        if ch=='trades': lag[ch].append(now*1000-d[-1]['time'])
    elif ch not in ('subscriptionResponse',): print('other',str(m)[:200])
ws.close()
for ch,a in arr.items():
    gaps=[b-a_ for a_,b in zip(a,a[1:])]
    print(ch,'msgs',len(a),'median gap s',round(statistics.median(gaps),3) if gaps else None,'min',round(min(gaps),3) if gaps else None,'max',round(max(gaps),3) if gaps else None,'levels',lv.get(ch),'median(local_recv - msg_time) ms',round(statistics.median(lag[ch])) if lag[ch] else None)
