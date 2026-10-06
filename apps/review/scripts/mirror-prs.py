#!/usr/bin/env python3
"""Mirror the open upstream backlog, preserving code, source attribution and fork isolation.
Run with authenticated gh. Normal pushes only. A durable manifest makes retries idempotent.
"""
import argparse,datetime,json,os,subprocess,time
from pathlib import Path

def run(args,input=None):
    r=subprocess.run(args,input=input,text=True,capture_output=True)
    if r.returncode: raise RuntimeError(r.stderr.strip() + '\n' + r.stdout.strip())
    return r.stdout

def api(path,method='GET',body=None):
    args=['gh','api',path,'--method',method]
    if body is not None: args+=['--input','-']
    for attempt in range(7):
        try: return json.loads(run(args,json.dumps(body) if body is not None else None) or 'null')
        except RuntimeError as e:
            if not any(t in str(e).lower() for t in ['rate limit','secondary','502','503','504']): raise
            delay=min(60,10*(attempt+1));print('GitHub throttled; waiting',delay,'seconds',flush=True);time.sleep(delay)
    raise RuntimeError('GitHub unavailable after retries')

def pages(path):
    out=[]
    for n in range(1,101):
        rows=api(f'{path}{"&" if "?" in path else "?"}per_page=100&page={n}')
        out+=rows
        if len(rows)<100:return out
    raise RuntimeError('Pagination exceeds 100 pages')

parser=argparse.ArgumentParser();parser.add_argument('--output',required=True);parser.add_argument('--git-dir',required=True);parser.add_argument('--source-git',default=str(Path(__file__).resolve().parents[3]));args=parser.parse_args()
out=Path(args.output);out.mkdir(parents=True,exist_ok=True);repo=Path(args.git_dir)
if not repo.exists(): run(['git','clone','--bare','--shared',args.source_git,str(repo)])
def git(*args,input=None):return run(['git','--git-dir',str(repo),*args],input)
git('remote','set-url','origin','https://github.com/katulevskiy/zeron.git')
try:git('remote','add','source','https://github.com/zeronsh/zeron.git')
except RuntimeError:pass
upstream=pages('repos/zeronsh/zeron/pulls?state=open');existing=pages('repos/katulevskiy/zeron/pulls?state=all')
manifest={};path=out/'mirrors.json'
if path.exists():manifest=json.loads(path.read_text())
for pr in existing:
    body=pr.get('body') or ''
    marker='<!-- contribution-manager-mirror:'
    if marker in body:
        n=body.split(marker)[1].split(' -->')[0]
        manifest[n]={'number':pr['number'],'url':pr['html_url'],'head':pr['head']['sha']}
base='mirror/upstream-main'
# Create a stable isolated base. Existing mirrors retain their base snapshot on reruns.
remote_base=git('ls-remote','origin','refs/heads/'+base).strip()
if remote_base:
    git('fetch','origin',f'refs/heads/{base}:refs/heads/{base}')
else:
    git('fetch','source',f'refs/heads/main:refs/heads/{base}')
    git('push','origin',f'refs/heads/{base}:refs/heads/{base}')
missing=[pr for pr in upstream if str(pr['number']) not in manifest]
if missing:
    git('fetch','source',*[f'refs/pull/{pr["number"]}/head:refs/heads/source-pr-{pr["number"]}' for pr in missing])
    refs=[]
    remote_refs={line.split()[1]:line.split()[0] for line in git('ls-remote','--heads','origin').splitlines()}
    for pr in missing:
        n=pr['number'];source=git('rev-parse',f'refs/heads/source-pr-{n}').strip()
        tree=git('rev-parse',source+'^{tree}').strip();branch=f'mirror/upstream-pr/{n}'
        parents=['-p',source]
        remote=remote_refs.get('refs/heads/'+branch)
        if remote:
            git('fetch','origin',f'refs/heads/{branch}:refs/heads/{branch}')
            parents+=['-p',git('rev-parse',f'refs/heads/{branch}').strip()]
        sha=git('-c','user.name=Contribution Manager','-c','user.email=contribution-manager@zeron.exnomic.com','commit-tree',tree,*parents,input=f'Mirror upstream PR #{n} without triggering bulk CI [skip ci]\n\nSource: {pr["html_url"]}\nSource revision: {source}\n').strip()
        git('update-ref',f'refs/heads/{branch}',sha);refs.append(f'refs/heads/{branch}:refs/heads/{branch}')
    # Normal push cannot overwrite unrelated history.
    git('push','origin',*refs)
for index,pr in enumerate(upstream):
    n=str(pr['number'])
    if n not in manifest:
        body=f'<!-- contribution-manager-mirror:{n} -->\n## Upstream mirror\n\nSource: {pr["html_url"]}\nOriginal author: @{pr["user"]["login"]}\nOriginal revision: `{pr["head"]["sha"]}`\n\nThis PR is an isolated Contribution Manager pilot mirror. Its reviews, checks and merge state are separate from upstream. An empty `[skip ci]` commit suppresses automatic bulk CI; the code tree matches the imported upstream revision.\n\n---\n\n'+(pr.get('body') or '')
        mirrored=api('repos/katulevskiy/zeron/pulls','POST',{'title':pr['title'],'body':body[:65000],'head':f'mirror/upstream-pr/{n}','base':base,'draft':pr['draft']})
        manifest[n]={'number':mirrored['number'],'url':mirrored['html_url'],'head':mirrored['head']['sha']}
        path.write_text(json.dumps(manifest,indent=2)+'\n');time.sleep(2)
    print(f'[{index+1}/{len(upstream)}] upstream #{n} → fork #{manifest[n]["number"]}',flush=True)
(out/'upstream-open-prs.json').write_text(json.dumps({'repository':'zeronsh/zeron','fetchedAt':datetime.datetime.now(datetime.timezone.utc).isoformat(),'pullRequests':upstream},indent=2)+'\n')
subprocess.run(['python3',str(Path(__file__).with_name('preserve-mirror-bases.py')),'--directory',str(out),'--git-dir',str(repo)],check=True)
print('Mirrored',len(upstream),'open upstream PRs. Manifest:',path,flush=True)
