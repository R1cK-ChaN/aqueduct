import sys, site; sys.path.append(site.getusersitepackages())
import json, collections, datetime
import lz4.frame, pyarrow.parquet as pq
d=sys.argv[1]
cp=pq.read_table(d+'/l4_checkpoints.parquet').to_pylist()
for c in cp:
    data=json.loads(c['data'])
    print('checkpoint', c['timestamp'], 'last_block', c['last_block_number'], 'type', type(data).__name__, 'len', len(data), 'side lens', [len(x) for x in data][:4])
    e=data[0][0]; print('  first bid entry:', json.dumps(e)[:420])
    e=data[1][0]; print('  first ask entry:', json.dumps(e)[:200])
    trig=sum(1 for s in data for u,o in s if o.get('isTrigger')); print('  trigger orders', trig, 'tifs', collections.Counter(o.get('tif') for s in data for u,o in s).most_common(5))
t=pq.read_table(d+'/l4_orderbook.parquet')
print('diff types', collections.Counter(t.column('diff_type').to_pylist()).most_common())
bn=t.column('block_number').to_pylist(); print('blocks', bn[0], bn[-1], 'distinct', len(set(bn)))
print('timestamp==block_time all?', t.column('timestamp').equals(t.column('block_time')))
o=pq.read_table(d+'/l4_orders.parquet'); print('order statuses', collections.Counter(o.column('status').to_pylist()).most_common(12))
if len(sys.argv)>2:
    raw=lz4.frame.decompress(open(sys.argv[2],'rb').read()); ts=[]
    for l in raw.split(b'\n'):
        if l.strip(): ts.append(json.loads(l)['raw']['data']['time'])
    f=lambda x: datetime.datetime.utcfromtimestamp(x/1000).strftime('%H:%M:%S.%f')[:-3]
    print('official snaps in hour', len(ts), 'first', f(ts[0]), 'last', f(ts[-1]))
    w=[x for x in ts if 1794322800000<=x<1794323700000]; print('in 15:00-15:15:', len(w), [f(x) for x in w][:40])
