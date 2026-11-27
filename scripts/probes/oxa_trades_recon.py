import sys, site; sys.path.append(site.getusersitepackages())
import json, os, datetime, collections
import lz4.frame
import pyarrow.parquet as pq
coin=sys.argv[1]; pqpath=sys.argv[2]; fills=sys.argv[3:]
off={}; dupkeys=0; tmin=None; tmax=None; nblocks=0
for p in fills:
    raw=lz4.frame.decompress(open(p,'rb').read())
    for l in raw.split(b'\n'):
        if not l.strip(): continue
        if coin.encode() not in l: 
            nblocks+=1; continue
        o=json.loads(l); nblocks+=1
        for user,f in o['events']:
            if f['coin']!=coin: continue
            k=(f['tid'],user,f['oid'],f['side'])
            if k in off: dupkeys+=1
            f['_bt']=o['block_time']; f['_bn']=o['block_number']
            off[k]=f
            t=f['time']; tmin=t if tmin is None or t<tmin else tmin; tmax=t if tmax is None or t>tmax else tmax
print(coin,'official rows',len(off),'dup keys',dupkeys,'time range',tmin,tmax,'blocks',nblocks)
t=pq.read_table(pqpath).to_pylist()
hours=set(int(os.path.basename(p).split('.')[0]) for p in fills)
ox={}; oxdup=0
for r in t:
    ts=int(r['timestamp'].timestamp()*1000+0.5)
    if r['timestamp'].hour not in hours: continue
    side='B' if r['side'] in ('B','buy','Buy') else 'A'
    k=(r['trade_id'],r['user_address'],r['order_id'],side)
    if k in ox: oxdup+=1
    r['_ts']=ts; ox[k]=r
print('0xArchive rows in same hours',len(ox),'dup keys',oxdup,'sides seen',collections.Counter(r['side'] for r in t).most_common(4))
only_off=[k for k in off if k not in ox]; only_ox=[k for k in ox if k not in off]
print('only in official',len(only_off),'only in 0xArchive',len(only_ox))
for k in only_off[:3]: print('  off-only',k,off[k]['time'],off[k]['px'],off[k]['sz'],off[k]['_bt'])
for k in only_ox[:3]: print('  oxa-only',k,ox[k]['_ts'],ox[k]['price'],ox[k]['size'])
bad=collections.Counter(); n=0
def feq(a,b,tol=1e-9): return abs(float(a)-float(b))<=tol*max(1.0,abs(float(a)))
for k,f in off.items():
    r=ox.get(k)
    if r is None: continue
    n+=1
    if f['time']!=r['_ts']: bad['time']+=1
    if not feq(f['px'],r['price']): bad['px']+=1
    if not feq(f['sz'],r['size']): bad['sz']+=1
    if bool(f['crossed'])!=bool(r['crossed']): bad['crossed']+=1
    if not feq(f['fee'],r['fee'],1e-7): bad['fee']+=1
    if not feq(f['closedPnl'],r['closed_pnl'],1e-7): bad['closed_pnl']+=1
    if not feq(f['startPosition'],r['start_position'],1e-7): bad['start_position']+=1
    if f['hash']!=r['tx_hash']: bad['hash']+=1
    if f['dir']!=r['direction']: bad['dir']+=1
    if (f.get('cloid') or '')!=(r['cloid'] or ''): bad['cloid']+=1
    if not feq(f.get('deployerFee') or 0,r['deployer_fee'] or 0,1e-7): bad['deployer_fee']+=1
    if not feq(f.get('priorityGas') or 0,r['priority_gas'] or 0,1e-7): bad['priority_gas']+=1
print('matched rows',n,'field mismatches',dict(bad))
ntl=sum(float(f['px'])*float(f['sz']) for f in off.values())/2
print('official notional $M (one-sided)',round(ntl/1e6,2),'liquidation-tagged official rows',sum(1 for f in off.values() if 'liquidation' in f))
