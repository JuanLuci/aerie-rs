"""Lists the block changes and positioned sounds a probe capture holds
after the probe joined, relative to an origin: one line per packet, a
single change as `set x y z block filler rotation`, a section's batch
as `batch` with its changes sorted. Run the same scenario on the
official server and on Aerie, each around its own origin, and diff the
two outputs.

    python3 tools/diff/cells.py C ox,oy,oz [radius]

Changes further than `radius` cells from the origin (16 by default) are
left out: the world around changes by itself (grass, crops).

This is how connected blocks (spec 23) are compared: a recorded
placement chain is sent once per block of a layout, moved to each cell
and turned to each yaw, and both servers must answer with the same
blocks, rotations and part cells after every placement.
"""
import re,sys,pathlib
sys.path.insert(0,str(pathlib.Path(__file__).resolve().parent))
import cap
o=[int(v) for v in sys.argv[2].split(',')]; R=int(sys.argv[3]) if len(sys.argv)>3 else 16
near=lambda c: all(abs(v)<=R for v in c[:3])
joined=False
for k,i,name,t in cap.packets(sys.argv[1],(100,140,141,155)):
    if i==100: joined=True; continue
    if not joined: continue
    g=lambda n,s=None: int(m.group(1)) if (m:=re.search(r'\b%s: (-?\d+)'%n,s or t)) else 0
    if i==155:
        p=re.search(r'x: ([-\d.]+),\s*y: ([-\d.]+),\s*z: ([-\d.]+)',t)
        c=[round(float(p.group(j+1))-o[j],3) for j in range(3)]
        if near(c): print('sound',g('sound_event_index'),*c)
    elif i==140:
        c=(g('x')-o[0],g('y')-o[1],g('z')-o[2],g('block_id'),g('filler'),g('rotation'))
        if near(c): print('set',*c)
    else:
        cells=[]
        for m2 in re.finditer(r'SetBlockCmd \{(.*?)\}',t,re.S):
            b=m2.group(1); n=g('index',b)
            cells.append((g('x')*32+(n&31)-o[0],g('y')*32+(n>>10)-o[1],g('z')*32+((n>>5)&31)-o[2],g('block_id',b),g('filler',b),g('rotation',b)))
        cells=sorted(c for c in cells if near(c))
        if cells: print('batch',' | '.join(' '.join(str(v) for v in c) for c in cells))
