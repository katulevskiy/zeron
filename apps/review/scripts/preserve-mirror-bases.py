#!/usr/bin/env python3
"""Restore each mirror's original stacked base without changing unrelated branches."""
import argparse,json,subprocess,time
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--directory',required=True);p.add_argument('--git-dir',required=True);a=p.parse_args();d=Path(a.directory)
def run(args):
 r=subprocess.run(args,capture_output=True,text=True,check=True);return r.stdout
manifest=json.loads((d/'mirrors.json').read_text());raw=json.loads((d/'upstream-open-prs.json').read_text())['pullRequests']
bases={pr['base']['ref']for pr in raw if pr['base']['ref']!='main'}
for base in sorted(bases):
 mirror='mirror/upstream-base/'+base
 run(['git','--git-dir',a.git_dir,'fetch','source',f'refs/heads/{base}:refs/heads/{mirror}'])
 run(['git','--git-dir',a.git_dir,'push','origin',f'refs/heads/{mirror}:refs/heads/{mirror}'])
 for pr in raw:
  if pr['base']['ref']!=base:continue
  number=manifest[str(pr['number'])]['number']
  run(['gh','api',f'repos/katulevskiy/zeron/pulls/{number}','--method','PATCH','-f',f'base={mirror}'])
  print('Restored stacked base for upstream',pr['number'],'→ fork',number,mirror,flush=True);time.sleep(2)
