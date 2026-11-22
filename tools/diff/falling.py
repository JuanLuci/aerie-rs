"""Shows what a probe capture holds of a falling block (spec 24): the
block's removal, its particle, the entity's components and one line per
transform, the landing's particles and sound, the removal of the entity
and the items dropped next to it. Positions are relative to the origin
given, so the output of the official server and of Aerie can be compared
line by line.

Heights are not printed: ticks last a thirtieth of a second, or as long
as they really took when they ran late, on both servers. The last line
replays the rule of the spec over the capture instead and says whether
the tick length each move implies is a real one.

    python3 tools/diff/falling.py C ox,oy,oz HEIGHT [AREA]

The scenario, as chat lines of the probe (x y z is the origin, a free
cell on a floor; the block and height are free to choose; AREA is the
width times depth of the block's box, 0.64 for the barrel):

    /block set x y+HEIGHT z Debug_Falling_Barrel
    /block setticking x y+HEIGHT z
"""
import re,sys,pathlib
sys.path.insert(0,str(pathlib.Path(__file__).resolve().parent))
import cap
o=[int(v) for v in sys.argv[2].split(',')]; H=int(sys.argv[3]); AREA=float(sys.argv[4]) if len(sys.argv)>4 else 0.64
recs=[]; start=None
def block(c,b):
    # one changed cell: a single change, or one of a section's batch
    global start
    if c==(0,H,0) and b==0 and start is None and any(r[0]=='set' and r[1][:3]==c for r in recs): start=len(recs)
    if abs(c[0])<9 and abs(c[2])<9: recs.append(('set',c+(b,)))
for k,i,name,t in cap.packets(sys.argv[1],(140,141,153,155,161)):
    g=lambda n,s=None: int(re.search(r'\b%s: (-?\d+)'%n,s or t).group(1))
    if i==140: block((g('x')-o[0],g('y')-o[1],g('z')-o[2]),g('block_id'))
    elif i==141:
        cells=[]
        for m in re.finditer(r'SetBlockCmd \{(.*?)\}',t,re.S):
            n=g('index',m.group(1))
            cells.append(((g('x')*32+(n&31)-o[0],g('y')*32+(n>>10)-o[1],g('z')*32+((n>>5)&31)-o[2]),g('block_id',m.group(1))))
        for c,b in sorted(cells): block(c,b)
    elif i==153:
        m=re.search(r'block_id: (\d+).*?BlockParticleEvent\(\s*(\d+).*?x: ([-\d.]+),\s*y: ([-\d.]+),\s*z: ([-\d.]+)',t,re.S)
        recs.append(('particle',int(m.group(1)),int(m.group(2)),tuple(round(float(m.group(3+j))-o[j],6) for j in range(3))))
    elif i==155:
        m=re.search(r'sound_event_index: (\d+).*?x: ([-\d.]+),\s*y: ([-\d.]+),\s*z: ([-\d.]+)',t,re.S)
        recs.append(('sound',int(m.group(1)),tuple(round(float(m.group(2+j))-o[j],6) for j in range(3))))
    else:
        top=re.match(r'\s*EntityUpdates \{\s*removed: (None|Some\(\s*\[([^\]]*)\],?\s*\)),',t)
        if top and top.group(2): recs.append(('removed',[int(v) for v in re.findall(r'\d+',top.group(2))]))
        for e in re.split(r'\n            EntityUpdate \{',t)[1:]:
            nid=int(re.search(r'network_id: (\d+)',e).group(1))
            m=re.search(r'x: ([-\d.e]+),\s*y: ([-\d.e]+),\s*z: ([-\d.e]+)',e)
            pos=tuple(float(m.group(j+1))-o[j] for j in range(3)) if m else None
            kinds=re.findall(r'(\w+Update)\(\s*\w+ \{',e)
            extra=re.findall(r'(block_id: \d+|entity_scale: [\d.]+|hitbox_collision_config_index: \d+|yaw: [-\d.]+|pitch: [-\d.]+|roll: [-\d.]+|quantity: \d+)',e)
            recs.append(('ent',nid,kinds,pos,extra))
if start is None: sys.exit('the block at the given height was never set and removed')
# The removal's particle and block change come in either order.
at=lambda r: r[0]=='particle' and r[2]==7 and r[3][0]==0.5 and r[3][2]==0.5 and r[3][1]>=H
end=next((n for n in range(start,len(recs)) if recs[n][0]=='ent' and 'BlockUpdate' in recs[n][2]),len(recs))
while start and at(recs[start-1]): start-=1
head=[r for r in recs[start:end] if r[0]=='set' or at(r)]
for r in sorted(head,key=lambda r:(r[0]!='set',r)): print(*r)
recs=[r for r in recs[start:end] if r not in head]+recs[end:]; start=0
mine=set(); items=set()
for r in recs[start:]:
    if r[0]=='ent' and 'BlockUpdate' in r[2]: mine.add(r[1])
    if r[0]=='ent' and 'ItemUpdate' in r[2] and r[3] and abs(r[3][0]-1)<3 and abs(r[3][2]-1)<3: items.add(r[1])
ys=[]
for r in recs[start:]:
    if r[0]=='set' or r[0] in ('particle','sound') and abs(r[-1][0])<9 and abs(r[-1][2])<9: print(*r)
    elif r[0]=='removed':
        if mine&set(r[1]): print('entity removed')
    elif r[1] in mine:
        ys.append(r[3][1]); print('entity',' '.join(r[2]),(r[3][0],r[3][2]),' '.join(r[4]))
    elif r[1] in items and 'ItemUpdate' in r[2]: print('item',' '.join(x for x in r[4] if 'quantity' in x),tuple(round(v,3) for v in r[3]))
# Each tick moves by the speed, then the speed gains gravity (mass 0.5,
# drag coefficient 0.05).
term=-(64*0.5*1000/(0.001225*AREA*1e6*0.05))**0.5
sg=lambda x:(x>0)-(x<0)
v=0.0; dts=[]
for a,b in zip(ys,ys[1:]):
    d=b-a
    if v==0.0:
        dt=(-d/32)**0.5; v=-32*dt   # the first tick only gained speed
    dt=d/v; dts.append(dt*1e6)
    v=max(v+32*(sg(term)-sg(v)*abs(v/term)**3)*dt,term)
if not dts: sys.exit('the entity never moved')
usual=sorted(dts)[len(dts)//2]; late=sum(1 for d in dts if d>usual+1)
ok=abs(usual-1e6/30)<1 and min(dts)>usual-1 and max(dts)<100000
print('moves',len(dts),'the fall follows the rule' if ok else 'THE FALL DOES NOT FOLLOW THE RULE: tick lengths %s'%[round(d,1) for d in dts],'(%d late ticks)'%late if ok and late else '')
