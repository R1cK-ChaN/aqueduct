import sys, site; sys.path.append(site.getusersitepackages())
import sys, json, os, statistics as st
import lz4.frame
def q(a,p):
    a=sorted(a); return a[min(len(a)-1,int(len(a)*p))]
first=True
for path in sys.argv[1:]:
    raw=lz4.frame.decompress(open(path,'rb').read())
    lines=raw.split(b'\n'); lines=[l for l in lines if l.strip()]
    ts=[]; nb=[]; na=[]; keys=None; vers=set(); bad=0; crossed=0; outer=[]
    for l in lines:
        try: o=json.loads(l)
        except Exception: bad+=1; continue
        if keys is None:
            keys=(list(o.keys()), list(o.get('raw',{}).keys()), list(o.get('raw',{}).get('data',{}).keys()))
            sample=l[:420].decode('utf8','replace')
        vers.add(o.get('ver_num'))
        d=o['raw']['data']; ts.append(d['time']); b,a=d['levels']; nb.append(len(b)); na.append(len(a)); outer.append(o.get('time'))
        if b and a and float(b[0]['px'])>=float(a[0]['px']): crossed+=1
    iv=[ts[i]-ts[i-1] for i in range(1,len(ts))]
    neg=sum(1 for x in iv if x<0); dup=sum(1 for x in iv if x==0)
    if first:
        print('schema keys:',keys); print('sample:',sample); first=False
    print(os.path.basename(path), 'bytes',os.path.getsize(path),'raw',len(raw),'ratio',round(len(raw)/os.path.getsize(path),1),'snaps',len(ts),'bad',bad,'ver',vers,
          'iv_ms p10/p50/p90/max',q(iv,.1),q(iv,.5),q(iv,.9),max(iv),'neg',neg,'dup',dup,'levels bid p50/min',q(nb,.5),min(nb),'ask p50/min',q(na,.5),min(na),'crossed',crossed,'span_s',round((ts[-1]-ts[0])/1000),'outer0',outer[0],'t0',ts[0])
