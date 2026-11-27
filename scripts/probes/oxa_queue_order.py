import sys, site; sys.path.append(site.getusersitepackages())
import json, collections
from decimal import Decimal as D
import pyarrow.parquet as pq
d=sys.argv[1]
cps=pq.read_table(d+'/l4_checkpoints.parquet').to_pylist()
def load(c):
    data=json.loads(c['data']); book={}
    for s in data:
        for u,o in s:
            if not o.get('isTrigger'): book[o['oid']]=(o['side'],D(o['limitPx']))
    return book
book=load(cps[0])
diffs=[x for x in pq.read_table(d+'/l4_orderbook.parquet').to_pylist() if x['block_number']>cps[0]['last_block_number']]
diffs.sort(key=lambda x:(x['block_number'],x['seq']))
targets={c['last_block_number']:c for c in cps[1:]}
def by_level(b):
    lv=collections.defaultdict(list)
    for oid,(side,px) in b.items(): lv[(side,px)].append(oid)
    return lv
i=0
while i<len(diffs):
    bn=diffs[i]['block_number']
    while i<len(diffs) and diffs[i]['block_number']==bn:
        x=diffs[i]; i+=1
        if x['diff_type']=='remove': book.pop(x['oid'],None)
        elif x['diff_type']=='new': book[x['oid']]=(x['side'],D(repr(x['price'])))
    if bn in targets:
        ref=by_level(load(targets[bn])); mine=by_level(book)
        multi=[k for k,v in ref.items() if len(v)>1]; diff=[k for k in multi if mine.get(k)!=ref[k]]
        moved=0; tot=0
        for k in multi:
            a=ref[k]; b=mine.get(k,[]); tot+=len(a); moved+=sum(1 for p,q in zip(a,b) if p!=q)
        print('checkpoint block',bn,'levels with >1 order',len(multi),'levels whose queue order differs from FIFO replay',len(diff),'orders at a different queue index',moved,'of',tot)
