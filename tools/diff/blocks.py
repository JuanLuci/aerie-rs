"""Compares two probe captures of tools/diff/tree-fall.txt (official server
and Aerie): block changes, batches, heights, particles and drops in the test
area, side by side.

    aerie-server probe --upstream <official> --capture A --chat "$(paste -sd';' tools/diff/tree-fall.txt)"
    aerie-server probe --upstream <aerie> --capture B --chat "..."
    python3 tools/diff/blocks.py A B

Needs the dump example: cargo build --release -p aerie-protocol --example dump.
"""
import subprocess,sys,os,re
import pathlib
D=str(pathlib.Path(__file__).resolve().parents[2]/'target'/'release'/'examples'/'dump')
def dump(d,f,i): return subprocess.run([D,str(i),os.path.join(d,f)],capture_output=True,text=True).stdout
def events(d):
    rows=[l.rstrip('\n').split('\t') for l in open(os.path.join(d,'index.tsv'))]
    files={int(f.split('-')[0]):f for f in os.listdir(d) if f.endswith('.bin')}
    out=[]; started=False
    for r in rows:
        n,i=int(r[0]),int(r[1])
        if i not in (140,141,153,183,184,161,155): continue
        t=dump(d,files[n],i)
        if i==140:
            m=re.search(r'x: (-?\d+),\s*y: (-?\d+),\s*z: (-?\d+),\s*block_id: (-?\d+)',t.replace('\n',' '))
            x,y,z,b=map(int,m.groups())
            if not(528<=x<=531 and 140<=y<=146 and 253<=z<=255): continue
            if (x,y,z)==(530,141,254) and b==0: started=True
            if started: out.append(f"SetBlock {x},{y},{z}={b}")
        elif not started: continue
        elif i==141:
            tt=t.replace('\n',' ')
            sec=re.search(r'x: (-?\d+),\s*y: (-?\d+),\s*z: (-?\d+),',tt).groups()
            cmds=re.findall(r'index: (-?\d+),\s*block_id: (-?\d+)',tt)
            if sec!=('16','4','7'): continue
            out.append("SetBlocks "+" ".join(f"{a}={b}" for a,b in cmds))
        elif i in (183,184):
            tt=t.replace('\n',' ')
            if i==183:
                x,z,h=re.search(r'x: (-?\d+),\s*z: (-?\d+),\s*height: (-?\d+)',tt).groups()
                if 528<=int(x)<=531 and 253<=int(z)<=255: out.append(f"Height {x},{z}={h}")
            else:
                sec=re.search(r'x: (-?\d+),\s*z: (-?\d+),',tt).groups()
                if sec==('16','7'):
                    out.append("Heights "+" ".join(f"{a}={b}" for a,b in re.findall(r'index: (-?\d+),\s*height: (-?\d+)',tt)))
        elif i==153:
            tt=t.replace('\n',' ')
            m=re.search(r'block_id: (-?\d+),.*?BlockParticleEvent\(\s*(\d+),.*?x: (-?[\d.]+),\s*y: (-?[\d.]+),\s*z: (-?[\d.]+)',tt)
            if m and 528<=float(m.group(3))<=532 and 140<=float(m.group(4))<=147: out.append(f"Particle {m.group(1)} ev{m.group(2)} {m.group(3)},{m.group(4)},{m.group(5)}")
        elif i==161:
            for it in re.findall(r'item_id: "([^"]+)"',t):
                out.append(f"Item {it}")
        elif i==155:
            out.append("Sound3D")
    return out
a=events(sys.argv[1]); b=events(sys.argv[2])
import itertools
w=max(len(x) for x in a+b+['x'])
for x,y in itertools.zip_longest(a,b,fillvalue=''):
    print(f"{x:<{w}}  {'==' if x==y else '!='}  {y}")
