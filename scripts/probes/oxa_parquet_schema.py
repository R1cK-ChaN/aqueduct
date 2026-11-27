import sys, site; sys.path.append(site.getusersitepackages())
import pyarrow.parquet as pq
for p in sys.argv[1:]:
    t=pq.read_table(p)
    print('=====', p.split('/')[-2], p.split('/')[-1], 'rows', t.num_rows)
    for f in t.schema: print('   ', f.name, f.type)
    r=t.slice(0,2).to_pylist()
    for x in r: print('   row:', {k:(str(v)[:110]) for k,v in x.items()})
