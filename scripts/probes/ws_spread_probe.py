import json, time, threading, math, sys, collections
import websocket
DUR=300
main=[r[0] for r in json.load(open('perps_rank.json'))[:45]]
h3=[r[0] for r in json.load(open('hip3_rank.json'))[:25]]
szdec={r[0]:r[2] for r in json.load(open('perps_rank.json'))}
szdec.update({r[0]:r[2]['szDecimals'] for r in json.load(open('hip3_rank.json'))})
coins=main+h3
st={c:dict(last=None,tw_spread=0.0,tw_bidsz=0.0,tw_asksz=0.0,tw=0.0,nbbo=0,ntr=0,ntl=0.0,spreads=[],mid0=None,mids=[]) for c in coins}
l2times=[]; l2dtimes=[]; lat=[]
t0=time.time()
def on_msg(ws,msg):
    now=time.time()
    m=json.loads(msg); ch=m.get('channel'); d=m.get('data')
    if ch=='bbo':
        c=d['coin']; s=st.get(c)
        if not s: return
        b,a=d['bbo']
        if not b or not a: return
        bp,ap=float(b['px']),float(a['px']); mid=(bp+ap)/2
        if s['last']:
            lt,lsp,lb,la=s['last']; dt=now-lt
            s['tw_spread']+=lsp*dt; s['tw_bidsz']+=lb*dt; s['tw_asksz']+=la*dt; s['tw']+=dt
        sp=(ap-bp)/mid*1e4
        s['last']=(now,sp,float(b['sz'])*bp,float(a['sz'])*ap); s['nbbo']+=1; s['spreads'].append(sp); s['mids'].append((now,mid)); s['px']=mid
        lat.append(now*1000-d['time'])
    elif ch=='trades':
        for t in d:
            s=st.get(t['coin'])
            if s and t['time']/1000>t0-1: s['ntr']+=1; s['ntl']+=float(t['px'])*float(t['sz'])
    elif ch=='l2Book': l2times.append((now,d['time']))
    elif ch=='l2': l2dtimes.append(now)
def on_open(ws):
    for c in coins:
        ws.send(json.dumps({'method':'subscribe','subscription':{'type':'bbo','coin':c}}))
        ws.send(json.dumps({'method':'subscribe','subscription':{'type':'trades','coin':c}}))
    ws.send(json.dumps({'method':'subscribe','subscription':{'type':'l2Book','coin':'BTC'}}))
    ws.send(json.dumps({'method':'subscribe','subscription':{'type':'l2','c':'BTC'}}))
ws=websocket.WebSocketApp('wss://api.hyperliquid.xyz/ws',on_message=on_msg,on_open=on_open)
th=threading.Thread(target=ws.run_forever,daemon=True); th.start()
time.sleep(DUR); ws.close()
el=time.time()-t0
def tick_bps(c,px):
    dec=max(0,6-szdec[c]); sig=10**(math.floor(math.log10(px))-4); t=max(sig,10**-dec)
    if px>=100000: t=1.0
    return t/px*1e4
out=[]
for c in coins:
    s=st[c]
    if s['tw']<=0: out.append(dict(coin=c,note='no bbo')); continue
    sp=sorted(s['spreads']); px=s['px']
    # 10s realized vol in bps
    mids=s['mids']; rets=[]; j=0
    grid=[]; t=mids[0][0]; k=0; last=mids[0][1]
    while t<mids[-1][0]:
        while k<len(mids) and mids[k][0]<=t: last=mids[k][1]; k+=1
        grid.append(last); t+=10
    for i in range(1,len(grid)): rets.append((grid[i]/grid[i-1]-1)*1e4)
    vol10=(sum(r*r for r in rets)/max(1,len(rets)))**0.5
    out.append(dict(coin=c,px=px,tw_spread_bps=round(s['tw_spread']/s['tw'],2),med_spread_bps=round(sp[len(sp)//2],2),tick_bps=round(tick_bps(c,px),2),bbo_per_s=round(s['nbbo']/el,2),trades_per_min=round(s['ntr']/el*60,1),ntl_per_min=round(s['ntl']/el*60),avg_trade=round(s['ntl']/max(1,s['ntr'])),tob_bid_usd=round(s['tw_bidsz']/s['tw']),tob_ask_usd=round(s['tw_asksz']/s['tw']),vol10s_bps=round(vol10,2)))
json.dump(out,open('wsprobe_out.json','w'))
iv=[l2times[i][0]-l2times[i-1][0] for i in range(1,len(l2times))]
iv2=[(l2times[i][1]-l2times[i-1][1]) for i in range(1,len(l2times))]
dv=[l2dtimes[i]-l2dtimes[i-1] for i in range(1,len(l2dtimes))]
def q(a,p): a=sorted(a); return round(a[int(len(a)*p)],3) if a else None
print('elapsed',round(el),'l2Book msgs',len(l2times),'interval s p10/p50/p90',q(iv,.1),q(iv,.5),q(iv,.9),'exch-time interval ms p50',q(iv2,.5))
print('l2 diff msgs',len(l2dtimes),'interval s p10/p50/p90',q(dv,.1),q(dv,.5),q(dv,.9))
print('bbo local-minus-exchange time ms p10/p50/p90',q(lat,.1),q(lat,.5),q(lat,.9))
print('%-14s %9s %7s %7s %6s %6s %7s %9s %7s %8s %8s %6s'%('coin','px','twSpr','medSpr','tick','bbo/s','tr/min','ntl/min','avgTr','bid$','ask$','vol10'))
for o in out:
    if 'note' in o: print(o); continue
    print('%-14s %9.5g %7.2f %7.2f %6.2f %6.2f %7.1f %9d %7d %8d %8d %6.2f'%(o['coin'],o['px'],o['tw_spread_bps'],o['med_spread_bps'],o['tick_bps'],o['bbo_per_s'],o['trades_per_min'],o['ntl_per_min'],o['avg_trade'],o['tob_bid_usd'],o['tob_ask_usd'],o['vol10s_bps']))
