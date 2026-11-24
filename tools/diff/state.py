"""Compares the player states two probe runs report (`state.json`, or one
labelled step of `states.jsonl`): prints one line per value that differs,
as `path: official | aerie`. Ids that differ between servers are already
names in the report; what always differs (the client id, packet counts,
how many sections were sent) is left out, and so is the position unless
`--position` is given, since the two runs usually play in different
places.

    python3 tools/diff/state.py RUN_A RUN_B [--label NAME] [--position]
        [--skip key,key]

RUN is a probe capture folder. With `--label`, the `state NAME` step of
each run's script is compared instead of the final state.
"""
import json,sys,pathlib
args=sys.argv[1:]
def opt(n):
    if n in args:
        i=args.index(n); v=args[i+1]; del args[i:i+2]; return v
def flag(n):
    if n in args: args.remove(n); return True
label=opt('--label'); skip=set(filter(None,(opt('--skip') or '').split(',')))
skip|={'client_id','packets','sections_known','script'}
if not flag('--position'): skip|={'position'}
def load(d):
    d=pathlib.Path(d)
    if label is None: return json.loads((d/'state.json').read_text())
    for l in (d/'states.jsonl').read_text().splitlines():
        s=json.loads(l)
        if s['label']==label: return s['state']
    sys.exit(f'{d}: no state step labelled {label!r}')
def walk(a,b,path,out):
    if isinstance(a,dict) and isinstance(b,dict):
        for k in sorted(set(a)|set(b)):
            if not path and k in skip: continue
            walk(a.get(k),b.get(k),path+[k],out)
    elif isinstance(a,list) and isinstance(b,list) and len(a)==len(b):
        for i,(x,y) in enumerate(zip(a,b)): walk(x,y,path+[str(i)],out)
    elif a!=b:
        out.append(f"{'.'.join(path)}: {json.dumps(a)} | {json.dumps(b)}")
out=[]; walk(load(args[0]),load(args[1]),[],out)
print('\n'.join(out) if out else 'same')
sys.exit(1 if out else 0)
