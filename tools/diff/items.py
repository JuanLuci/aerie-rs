"""Traces the item entities of one item in a probe capture: spawn,
transforms, stack changes, removals, tick by tick (ticks counted by
NetworkTick). Run it on captures of tools/diff/tree-pickup.txt from the
official server and from Aerie and compare.

    aerie-server probe --upstream <server> --capture C --chat "$(paste -sd';' tools/diff/tree-pickup.txt)" --goto "530.5 136 254.5"
    python3 tools/diff/items.py C [item]
"""
import subprocess,os,re,sys
import pathlib
D=str(pathlib.Path(__file__).resolve().parents[2]/'target'/'release'/'examples'/'dump')
d=sys.argv[1]; item=sys.argv[2] if len(sys.argv)>2 else 'Wood_Oak_Trunk'
rows=[l.split('\t') for l in open(os.path.join(d,'index.tsv'))]
files={int(f.split('-')[0]):f for f in os.listdir(d) if f.endswith('.bin')}
ids={}; ticks=0
for r in rows:
    n,i=int(r[0]),int(r[1])
    if i==5: ticks+=1
    if i!=161: continue
    t=subprocess.run([D,'161',os.path.join(d,files[n])],capture_output=True,text=True).stdout
    rem=re.search(r'removed: Some\(\s*\[([^\]]*)\]',t)
    if rem:
        for x in re.findall(r'\d+',rem.group(1)):
            if int(x) in ids: print(f"tick {ticks} #{n} {x} REMOVED")
    for blk in re.split(r'\n            EntityUpdate \{',t)[1:]:
        nid=int(re.search(r'network_id: (-?\d+)',blk).group(1))
        if f'item_id: "{item}"' in blk and nid not in ids: ids[nid]=1
        if nid not in ids: continue
        comps=re.findall(r'\n                        (\w+)\(',blk)
        pos=re.search(r'position: Some\(\s*Position \{\s*x: ([-\d.e]+),\s*y: ([-\d.e]+),\s*z: ([-\d.e]+)',blk)
        extra=''
        m=re.search(r'entity_scale: ([-\d.e]+)',blk)
        if m: extra+=f' scale={m.group(1)}'
        p='' if not pos else '(%.4f, %.4f, %.4f)'%tuple(map(float,pos.groups()))
        print(f"tick {ticks} #{n} id{nid} {comps} {p}{extra}")
