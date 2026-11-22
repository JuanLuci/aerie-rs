"""Lists what a server answers to interaction chains in a probe capture:
its chain state packets, cancels and active slot corrections, in order,
with root interactions by name (two servers can number them differently)
and block positions relative to the first one named (or to --origin).
Replay the same recorded chains
on the official server and on Aerie and compare the two listings.

    aerie-server probe --upstream <server> --capture C --name Prober \
        --chat "<setup commands>" --after "<recorded chain packets>"
    python3 tools/diff/chains.py C [--origin X,Y,Z] [--from N] [--final]

Which tick a client packet lands in differs from run to run, and with it
how the states are spread over packets. --final lists, per chain and fork,
the last state sent for every step and the chain's last state instead,
which does not depend on that.
"""
import subprocess,os,re,sys,pathlib
d=sys.argv[1]
def opt(name,default=None):
    return sys.argv[sys.argv.index(name)+1] if name in sys.argv else default
origin=[int(v) for v in opt('--origin').split(',')] if opt('--origin') else None
start=int(opt('--from','0'))
D=str(pathlib.Path(__file__).resolve().parents[2]/'target'/'release'/'examples'/'dump')
rows=[l.rstrip('\n').split('\t') for l in open(d+'/index.tsv')]
files={int(f.split('-')[0]):f for f in os.listdir(d) if f.endswith('.bin')}
def dump(k,i):
    return subprocess.run([D,str(i),os.path.join(d,files[k])],capture_output=True,text=True).stdout
names={}
for r in rows:
    if int(r[1])==67:
        t=dump(int(r[0]),67)
        for m in re.finditer(r'\n            \(\n                (\d+),\n                RootInteraction \{\n                    id: Some\(\n                        "([^"]+)"',t):
            names[m.group(1)]=m.group(2)
        break
final='--final' in sys.argv
chains={}
def note(u,path):
    """One chain update of a state packet, for --final."""
    head=u.split('interaction_data: Some(')[0].split('new_forks: Some(')[0]
    cid=re.search(r'chain_id: (-?\d+)',head)
    fk=head.split('forked_id: Some(')[1].split('data:')[0] if 'forked_id: Some(' in head else ''
    ids=[(int(a),0) for a in re.findall(r'entry_index: (\d+)',fk)]
    subs=re.findall(r'ForkedChainId \{(.*?)(?=ForkedChainId \{|$)',fk,re.S)
    ids=[(int(re.search(r'entry_index: (\d+)',x).group(1)) if 'entry_index' in x else 0,
          int(re.search(r'sub_index: (\d+)',x).group(1)) if 'sub_index' in x else 0) for x in subs]
    key=(int(cid.group(1)) if cid else 0,tuple(ids))
    c=chains.setdefault(key,{'steps':{},'state':None,'root':None})
    m=re.search(r'override_root_interaction: (\S+),',head)
    if m: c['root']=m.group(1)
    st=re.findall(r'\n {12}(?: {12})*state: InteractionState\(\s*(\d)',head)
    m=re.search(r'state: InteractionState\(\s*(\d),\s*\),\s*$',head.rstrip().rstrip(',')) or re.search(r'state: InteractionState\(\s*(\d)',head.split('data:')[-1])
    c['state']=m.group(1) if m else '0'
    base=re.search(r'operation_base_index: (\d+)',u.split('interaction_data: Some(')[0])
    base=int(base.group(1)) if base else 0
    if 'interaction_data: Some(' in u:
        body=u.split('interaction_data: Some(')[-1]
        n=0
        for m in re.finditer(r'\n +(None,|Some\(\s*InteractionSyncData \{(.*?)\n +\},\s*\),)',body,re.S):
            if m.group(2) is not None:
                d=re.sub(r'progress: [-\d.e]+,','',m.group(2))
                c['steps'][base+n]=re.sub(r'\s+',' ',d).strip()
            n+=1
def split_updates(t):
    """Top-level updates and the forks nested in them, flattened."""
    out=[]
    for u in re.split(r'\n        SyncInteractionChain \{',t)[1:]:
        parts=re.split(r'\n                    SyncInteractionChain \{',u)
        out.append(parts[0]+(u[u.index('operation_base_index'):] if len(parts)>1 and 'operation_base_index' in parts[-1] else ''))
        for f in parts[1:]:
            out.append(f.split('\n                    },')[0])
        if len(parts)>1:
            tail=parts[-1].split('\n                ],\n            ),',1)
            if len(tail)>1: out[-len(parts)]=parts[0]+tail[1]
    return out
seen=False
for r in rows:
    k,i=int(r[0]),int(r[1])
    if k<start or i not in (290,291,177): continue
    # Active slots sent at join are not answers to a chain.
    seen|=i!=177
    if not seen: continue
    t=dump(k,i)
    t=re.sub(r'(root_interaction: )(\d+)',lambda m:m.group(1)+names.get(m.group(2),m.group(2)),t)
    if origin is None:
        m=re.search(r'BlockPosition \{\s*x: (-?\d+),\s*y: (-?\d+),\s*z: (-?\d+),',t)
        if m: origin=[int(v) for v in m.groups()]
    for n,a in zip('xyz',origin or [0,0,0]):
        t=re.sub(r'(\b%s: )(-?\d+),'%n,lambda m,a=a:'%s%d,'%(m.group(1),int(m.group(2))-a),t)
    # Defaults say nothing.
    t='\n'.join(l for l in t.split('\n') if not re.search(r': (None|false|0|0\.0|-1|-1\.0|-2147483648|\[\]),$',l))
    if not final:
        print('==',r[2]); print(t)
    elif i==290:
        for u in split_updates(t): note(u,None)
    else:
        print('==',r[2],re.sub(r'\s+',' ',t))
if final:
    for key in sorted(chains):
        c=chains[key]
        print('chain',key[0],'fork',list(key[1]),'root',c['root'],'state',c['state'])
        for n in sorted(c['steps']): print('  ',n,c['steps'][n])
