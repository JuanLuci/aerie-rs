"""Lists the effect updates and predicted stat changes a probe capture
holds for the probe's own entity, each with the number of server ticks
since the first one, effects by name. Run the same scenario on the
official server and on Aerie and compare.

    python3 tools/diff/effects.py C
"""
import re,sys,pathlib
sys.path.insert(0,str(pathlib.Path(__file__).resolve().parent))
import cap
names={}
tick=0; t0=None; me=None
for k,i,name,t in cap.packets(sys.argv[1],(51,100,5,161)):
    if i==51 and not names:
        for m in re.finditer(r'\n            \(\n                (\d+),\n                EntityEffect \{\n                    id: Some\(\n                        "([^"]+)"',t): names[m.group(1)]=m.group(2)
    if i==100: me=re.search(r'client_id: (-?\d+)',t).group(1)
    if i==5: tick+=1
    if i!=161: continue
    for blk in re.split(r'\n            EntityUpdate \{',t)[1:]:
        if re.search(r'network_id: (-?\d+)',blk).group(1)!=me: continue
        for m in re.finditer(r'EntityEffectUpdate \{(.*?)\}',blk,re.S):
            if t0 is None: t0=tick
            u=re.sub(r'\s+',' ',m.group(1)).strip()
            u=re.sub(r'id: (\d+)',lambda x:'id: '+names.get(x.group(1),x.group(1)),u)
            print(tick-t0,u)
        if t0 is not None:
            for m in re.finditer(r'op: EntityStatOp\(\s*(\d),\s*\),\s*predictable: true,\s*value: ([-\d.]+)',blk): print(tick-t0,'stat op',m.group(1),m.group(2))
