"""Lists what a probe capture holds about the player list and name plates,
one line per event with the server tick (relative to the first player list
packet): the list packets (add, remove, ping update), the Ping packets
(how many of the last values are set) and the name plate updates of the
players (entities whose first name plate text starts with --prefix, `Prober`
by default), and server messages. Run the same scenario on the official
server and on Aerie and diff the two outputs.

    python3 tools/diff/playerlist.py CAPTURE [--prefix Prober] [--noping]
"""
import re,sys,pathlib
sys.path.insert(0,str(pathlib.Path(__file__).resolve().parent))
import cap
d=sys.argv[1]
prefix=sys.argv[sys.argv.index('--prefix')+1] if '--prefix' in sys.argv else 'Prober'
noping='--noping' in sys.argv
tick=None; base=None; known=set(); last_ping=None
def rel(): return '-' if tick is None or base is None else tick-base
for k,i,name,t in cap.packets(d,(5,3,224,225,227,161,210)):
    if i==5:
        tick=int(re.search(r'server_tick: (\d+)',t).group(1)); continue
    if i in (224,225,227) and base is None: base=tick
    if i==3:
        if noping: continue
        v=[int(x)>0 for x in re.findall(r'last_ping_value_\w+: (-?\d+)',t)]
        gap='' if last_ping is None or tick is None else ' +%d'%(tick-last_ping)
        last_ping=tick
        print('%6s Ping set=%s%s'%(rel(),''.join('1' if x else '0' for x in v),gap)); continue
    if i==224:
        for p in re.finditer(r'uuid: ([-0-9a-f]+),\s*username: (\w+\(\s*)?"?([^",]*)"?,?\s*\)?,?\s*world_uuid: (None|Some\(\s*[-0-9a-f]+)',t,re.S):
            pass
        ents=re.findall(r'uuid: ([-0-9a-f]+),\s*username: Some\(\s*"([^"]*)",\s*\),\s*world_uuid: (None|Some)\(?\s*[-0-9a-f]*,?\s*\)?,\s*ping: (\d+),\s*spectating: (\w+)',t,re.S)
        print('%6s Add %s'%(rel(),' '.join('%s(world=%s ping=%s spec=%s)'%(n,w,p,s) for u,n,w,p,s in ents))); continue
    if i==225:
        print('%6s Remove %s'%(rel(),' '.join(re.findall(r'[0-9a-f]{8}-[-0-9a-f]+',t)))); continue
    if i==227:
        print('%6s PingUpdate %s'%(rel(),' '.join('%s=%s'%(u[-2:],p) for u,p in re.findall(r'([0-9a-f]{8}-[-0-9a-f]+),\s*(\d+),',t)))); continue
    if i==210:
        m=re.search(r'message_id: Some\(\s*"([^"]+)"',t); r=re.search(r'raw_text: Some\(\s*"([^"]*)"',t)
        print('%6s Message %s'%(rel(),(m and m.group(1)) or (r and r.group(1)))); continue
    if i==161:
        for blk in re.split(r'\n\s*EntityUpdate \{',t)[1:]:
            nid=re.search(r'network_id: (-?\d+)',blk).group(1)
            m=re.search(r'NameplateUpdate \{\s*text: "([^"]*)"',blk)
            if m and m.group(1).startswith(prefix): known.add(nid)
            if m and nid in known: print('%6s Nameplate entity=%s text=%r'%(rel(),nid,m.group(1)))
            if nid in known:
                r=re.search(r'removed: Some\(\s*\[(.*?)\]',blk,re.S)
                if r and 'ComponentUpdateType(\n' in r.group(1) and re.search(r'ComponentUpdateType\(\s*0,',r.group(1)):
                    print('%6s NameplateRemoved entity=%s'%(rel(),nid))
