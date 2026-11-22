"""Lists the stat updates a probe capture holds for the probe's own entity
(or every entity with --all), one line per update: `stat:op=value` with
`p` for predicted ones and the modifier key for modifier ops, and the
command replies in between. Run the same commands on the official server
and on Aerie and compare; regeneration lines differ only by tick timing.

    aerie-server probe --upstream <server> --capture C --name Prober --listen 16 \
        --chat "$(paste -sd';' tools/diff/stats.txt)"
    python3 tools/diff/stats.py C

A sprint (stamina drain and the regeneration delay after it):

    aerie-server probe --upstream <server> --capture C --name Prober --listen 28 --sprint 3
"""
import re,sys,pathlib
sys.path.insert(0,str(pathlib.Path(__file__).resolve().parent))
import cap
d=sys.argv[1]
me=None; joined=False
for k,i,name,t in cap.packets(d,(100,210,161)):
    if i==100:
        me=re.search(r'client_id: (-?\d+)',t).group(1); joined=True
    if i==210 and k>100:
        m=re.search(r'message_id: Some\(\s*"([^"]+)"',t); v=re.findall(r'value: ([-\d.e]+),',t)
        print('MSG',m and m.group(1),v)
    if i==161:
        if 'EntityStatsUpdate' not in t: continue
        for blk in re.split(r'\n            EntityUpdate \{',t)[1:]:
            nid=re.search(r'network_id: (-?\d+)',blk).group(1)
            if nid!=me and '--all' not in sys.argv: continue
            m=re.search(r'EntityStatsUpdate \{\s*entity_stat_updates: \[(.*?)\n                                \],\n                            \}',blk,re.S)
            if not m: continue
            out=[]
            for e in re.finditer(r'\(\s*(\d+),\s*\[(.*?)\n                                        \],',m.group(1),re.S):
                ups=[]
                for u in re.split(r'EntityStatUpdate \{',e.group(2))[1:]:
                    u=re.sub(r'\s+',' ',u)
                    o=re.search(r'op: EntityStatOp\( (\d+)',u).group(1); p='p' if 'predictable: true' in u else ''
                    v=float(re.search(r'value: ([-\d.e]+)',u).group(1))
                    k=re.search(r'modifier_key: Some\( "([^"]+)"',u)
                    m=re.search(r'modifier: Some\( Modifier \{ target: ModifierTarget\( (\d) \), calculation_type: CalculationType\( (\d) \), amount: ([-\d.e]+)',u)
                    x=f'op{o}{p}={v:.4g}'
                    if k: x+=f'[{k.group(1)}'+(f' t{m.group(1)}c{m.group(2)} {float(m.group(3)):.4g}' if m else '')+']'
                    if 'modifiers: Some' in u: x+='M'
                    ups.append(x)
                out.append('%s:%s'%(e.group(1),'+'.join(ups)))
            print(('me' if nid==me else nid),' '.join(out))
