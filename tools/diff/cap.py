"""Reads a probe capture for the scripts here: one run of the `dump`
example decodes every packet with one of the wanted ids."""
import subprocess,pathlib,os
ROOT=pathlib.Path(__file__).resolve().parents[2]
def _dump():
    for profile in ('release','fast','debug'):
        p=ROOT/'target'/profile/'examples'/'dump'
        if p.exists(): return str(p)
    raise SystemExit('build it first: cargo build --release -p aerie-protocol --example dump')
def packets(capture,ids):
    """Yields (sequence, packet id, name, text) in capture order."""
    out=subprocess.run([_dump(),'--capture',capture,','.join(str(i) for i in ids)],capture_output=True,text=True).stdout
    for part in ('\n'+out).split('\n### ')[1:]:
        head,_,text=part.partition('\n')
        f=head.split(' ',2)
        yield int(f[0]),int(f[1]),(f[2] if len(f)>2 else ''),text
