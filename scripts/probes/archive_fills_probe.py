import sys, site; sys.path.append(site.getusersitepackages())
import json, os, collections
import lz4.frame
p=sys.argv[1]
raw=lz4.frame.decompress(open(p,'rb').read())
lines=[l for l in raw.split(b'\n') if l.strip()]
print('bytes',os.path.getsize(p),'raw',len(raw),'ratio',round(len(raw)/os.path.getsize(p),1),'blocks(lines)',len(lines))
o=json.loads(lines[0]); print('envelope keys',list(o.keys())); print('local_time',o.get('local_time'),'block_time',o.get('block_time'),'block_number',o.get('block_number'))
n=0; coins=collections.Counter(); keys=collections.Counter(); crossed=collections.Counter(); ntl=0.0; bn=[]; liq=0; dirs=collections.Counter(); sample=None; hip3=None; lag=[]
import datetime
def ts(s): 
    return datetime.datetime.fromisoformat(s[:26]).timestamp()
for l in lines:
    o=json.loads(l); bn.append(o['block_number'])
    try: lag.append(ts(o['local_time'])-ts(o['block_time']))
    except Exception: pass
    for ev in o['events']:
        user,f=ev; n+=1; coins[f['coin']]+=1; crossed[f.get('crossed')]+=1; dirs[f.get('dir')]+=1
        for k in f: keys[k]+=1
        ntl+=float(f['px'])*float(f['sz'])
        if 'liquidation' in f: liq+=1
        if sample is None: sample=ev
        if hip3 is None and ':' in f['coin']: hip3=ev
print('fill rows',n,'distinct coins',len(coins),'notional(both sides)/2 $M',round(ntl/2/1e6,1))
print('field counts',dict(keys))
print('crossed',dict(crossed)); print('top coins',coins.most_common(12))
print('hip3 coins',sum(1 for c in coins if ':' in c),'hip3 rows',sum(v for c,v in coins.items() if ':' in c),'spot(@) rows',sum(v for c,v in coins.items() if c.startswith('@') or '/' in c))
print('liquidation-tagged rows',liq); print('dirs',dirs.most_common(10))
print('block numbers: first,last,count,contiguous?',bn[0],bn[-1],len(bn),bn[-1]-bn[0]+1==len(bn))
lag.sort(); print('local-block time lag s p50/p99',round(lag[len(lag)//2],3),round(lag[int(len(lag)*.99)],3))
print('sample',json.dumps(sample)[:600]); print('hip3 sample',json.dumps(hip3)[:700])
