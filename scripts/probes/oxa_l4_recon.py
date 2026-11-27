import sys, site; sys.path.append(site.getusersitepackages())
import json, collections, datetime
from decimal import Decimal as D
import lz4.frame, pyarrow.parquet as pq
d=sys.argv[1]; coin=sys.argv[2]; fills_path=sys.argv[3]; off_l2=sys.argv[4] if len(sys.argv)>4 else None
ms=lambda dt: int(round(dt.timestamp()*1000))
cps=pq.read_table(d+'/l4_checkpoints.parquet').to_pylist()
def load_cp(c):
    data=json.loads(c['data']); book={}
    for s in data:
        for u,o in s:
            if o.get('isTrigger'): continue
            book[o['oid']]=(o['side'], D(o['limitPx']), D(o['sz']))
    return book
book=load_cp(cps[0]); start_block=cps[0]['last_block_number']
diffs=pq.read_table(d+'/l4_orderbook.parquet').to_pylist()
diffs=[x for x in diffs if x['block_number']>start_block]
diffs.sort(key=lambda x:(x['block_number'],x['seq']))
print(coin,'start checkpoint',cps[0]['timestamp'],'orders',len(book),'diffs to apply',len(diffs))
def dec(x): return D(repr(x)) if not isinstance(x,D) else x
# events to check
grid=pq.read_table(d+'/l2_orderbook.parquet').to_pylist()
t0=ms(cps[0]['timestamp']); t_end=ms(diffs[-1]['block_time'])
targets=[]  # (time_ms, kind, payload)
for g in grid:
    t=ms(g['timestamp'])
    if t0<t<=t_end: targets.append((t,'grid',g))
if off_l2:
    raw=lz4.frame.decompress(open(off_l2,'rb').read())
    for l in raw.split(b'\n'):
        if l.strip():
            o=json.loads(l)['raw']['data']
            if t0<o['time']<=t_end: targets.append((o['time'],'official',o))
for c in cps[1:]: targets.append((None,'cp',c))
# fills (maker side) by block
fills=collections.defaultdict(list)
raw=lz4.frame.decompress(open(fills_path,'rb').read())
for l in raw.split(b'\n'):
    if not l.strip() or coin.encode() not in l: continue
    o=json.loads(l)
    if o['block_number']<=start_block or o['block_number']>diffs[-1]['block_number']: continue
    for u,f in o['events']:
        if f['coin']==coin and not f['crossed']: fills[o['block_number']].append(f)
def agg(book, n=20):
    b=collections.defaultdict(lambda:[D(0),0]); a=collections.defaultdict(lambda:[D(0),0])
    for side,px,sz in book.values():
        x=(b if side=='B' else a)[px]; x[0]+=sz; x[1]+=1
    bids=sorted(b.items(), key=lambda kv:-kv[0])[:n]; asks=sorted(a.items(), key=lambda kv:kv[0])[:n]
    return [(px,v[0],v[1]) for px,v in bids],[(px,v[0],v[1]) for px,v in asks]
def levels(js):
    return [(D(x['px']),D(x['sz']),x['n']) for x in js]
def cmp(mine, theirs):
    n=min(len(mine),len(theirs)); px=sum(1 for i in range(n) if mine[i][0]==theirs[i][0]); full=sum(1 for i in range(n) if mine[i]==theirs[i])
    return n,px,full
res=collections.defaultdict(lambda:[0,0,0,0,0])  # kind -> snapshots, levels, px match, full match, perfect snapshots
gtargets=sorted([t for t in targets if t[0] is not None], key=lambda x:x[0])
cp_by_block={c['last_block_number']:c for c in cps[1:]}
gi=0; i=0; maker_ok=0; maker_bad=0; bad_examples=[]
grid_conv=collections.Counter()
while i<len(diffs):
    bn=diffs[i]['block_number']; bt=ms(diffs[i]['block_time'])
    # before applying block bn: evaluate targets with time < bt  (state after all blocks with block_time <= target time)
    while gi<len(gtargets) and gtargets[gi][0]<bt:
        t,kind,p=gtargets[gi]; gi+=1
        mb,ma=agg(book)
        if kind=='grid': tb,ta=levels(json.loads(p['bids'])),levels(json.loads(p['asks']))
        else: tb,ta=levels(p['levels'][0]),levels(p['levels'][1])
        r=res[kind]; r[0]+=1; ok=True
        for m,th in ((mb,tb),(ma,ta)):
            n,px,full=cmp(m,th); r[1]+=n; r[2]+=px; r[3]+=full; ok=ok and full==n and n>0
        r[4]+=ok
        if kind=='grid': grid_conv[len(tb)]+=1
    # maker fills in this block must rest in book before the block (or be placed in this same block)
    newoids=set()
    j=i
    while j<len(diffs) and diffs[j]['block_number']==bn:
        if diffs[j]['diff_type']=='new': newoids.add(diffs[j]['oid'])
        j+=1
    for f in fills.get(bn,[]):
        o=book.get(f['oid'])
        if (o is not None and o[1]==D(f['px'])) or f['oid'] in newoids: maker_ok+=1
        else:
            maker_bad+=1
            if len(bad_examples)<3: bad_examples.append((bn,f['oid'],f['px'],f['sz'],o))
    while i<j:
        x=diffs[i]; i+=1
        if x['diff_type']=='remove': book.pop(x['oid'],None)
        elif x['diff_type']=='new': book[x['oid']]=(x['side'],dec(x['price']),dec(x['new_size']))
        else:
            o=book.get(x['oid'])
            book[x['oid']]=(x['side'],dec(x['price']),dec(x['new_size']))
    if bn in cp_by_block:
        ref=load_cp(cp_by_block[bn]); same=sum(1 for k,v in ref.items() if book.get(k)==v)
        print('  checkpoint at block',bn,'ref orders',len(ref),'mine',len(book),'identical',same,'missing',len(set(ref)-set(book)),'extra',len(set(book)-set(ref)))
for kind,r in res.items():
    print(' ',kind,'snapshots',r[0],'levels compared',r[1],'price match',r[2],'px+sz+n match',r[3],'perfect snapshots',r[4])
print('  grid depth per side', dict(grid_conv))
print('  maker fills in window',maker_ok+maker_bad,'found resting at same price (or placed same block)',maker_ok,'not found',maker_bad,bad_examples)
