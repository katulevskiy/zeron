#!/usr/bin/env python3
"""Export real GitHub data for a pilot without deploying a workstation credential."""
import argparse,concurrent.futures,datetime,json,subprocess,time
from pathlib import Path

def api(path):
 for attempt in range(5):
  r=subprocess.run(['gh','api',path],capture_output=True,text=True)
  if r.returncode==0:return json.loads(r.stdout)
  if not any(code in r.stderr for code in ['502','503','504','rate limit']):raise RuntimeError(r.stderr)
  time.sleep(min(10,2*(attempt+1)))
 raise RuntimeError('GitHub API failed: '+path)
def pages(path):
 out=[]
 for page in range(1,101):
  data=api(path+('&'if'?'in path else'?')+f'per_page=100&page={page}')
  out+=data
  if len(data)<100:return out
 raise RuntimeError('Pagination limit')
parser=argparse.ArgumentParser();parser.add_argument('--directory',required=True);args=parser.parse_args();directory=Path(args.directory)
mirrors=json.loads((directory/'mirrors.json').read_text())
reverse={v['number']:int(k)for k,v in mirrors.items()}
originals=json.loads((directory/'upstream-open-prs.json').read_text())['pullRequests']
parents={pr['head']['ref']:pr['number'] for pr in originals if pr['head']['repo'] and pr['head']['repo']['full_name']=='zeronsh/zeron'}
raws=pages('repos/katulevskiy/zeron/pulls?state=open')
stamp=datetime.datetime.now(datetime.timezone.utc).isoformat()
def context(repository,raw):
 n=raw['number'];sha=raw['head']['sha'];prefix=f'repos/{repository}'
 reviews=pages(f'{prefix}/pulls/{n}/reviews')
 checks=[]
 for page in range(1,20):
  result=api(f'{prefix}/commits/{sha}/check-runs?filter=latest&per_page=100&page={page}')['check_runs']
  checks += [{'name':c['name'],'conclusion':c.get('conclusion')or'pending','url':c['html_url'],'revision':sha,'baseRevision':None,'runId':c['id']}for c in result]
  if len(result)<100:break
 statuses=api(f'{prefix}/commits/{sha}/status?per_page=100').get('statuses',[])
 checks += [{'name':s['context'],'conclusion':s['state'],'url':s.get('target_url'),'revision':sha,'baseRevision':None}for s in statuses]
 converted=[{'actor':r['user']['login'],'revision':r['commit_id'],'verdict':{'APPROVED':'pass','CHANGES_REQUESTED':'changes','DISMISSED':'dismissed'}.get(r['state'],'comment'),'summary':r.get('body')or'Native GitHub review','url':r['html_url'],'at':r['submitted_at'],'source':'github-review','sourceId':r['id']}for r in reviews]
 return {'revision':sha,'author':raw['user']['login'],'checks':checks,'reviews':converted,'fetchedAt':stamp}
def export(raw):
 n=raw['number'];details=api(f'repos/katulevskiy/zeron/pulls/{n}');c=context('katulevskiy/zeron',details)
 item={'id':f'pr:{n}','number':n,'kind':'pr','title':details['title'],'body':details.get('body'),'author':details['user']['login'],'url':details['html_url'],'createdAt':details['created_at'],'updatedAt':details['updated_at'],'revision':details['head']['sha'],'baseRevision':details['base']['sha'],'headBranch':details['head']['ref'],'baseBranch':details['base']['ref'],'draft':details['draft'],'merged':details['merged_at']is not None,'closed':details['state']=='closed','labels':[l['name']for l in details['labels']],'nativeReviews':c['reviews'],'checks':c['checks'],'mergeable':details['mergeable'],'mergeState':details['mergeable_state'],'additions':details['additions'],'deletions':details['deletions'],'changedFiles':details['changed_files'],'diffUrl':details['html_url']+'/files','syncedAt':stamp,'sample':False}
 if n in reverse:
  original=api(f'repos/zeronsh/zeron/pulls/{reverse[n]}')
  item['originalAuthor']=original['user']['login'];item['upstreamNumber']=reverse[n];item['upstreamUrl']=original['html_url'];item['upstream']=context('zeronsh/zeron',original)
  parent=parents.get(original['base']['ref']);item['dependencies']=[{'number':mirrors[str(parent)]['number'],'url':mirrors[str(parent)]['url'],'upstreamNumber':parent}]if parent and str(parent)in mirrors else[]
  item['upstreamBase']=original['base']['ref'];item['upstream']['state']=original['state'];item['upstream']['merged']=original['merged_at']is not None
 return item
items=[]
with concurrent.futures.ThreadPoolExecutor(max_workers=4)as pool:
 for item in pool.map(export,raws):
  items.append(item)
  if len(items)%20==0:print('Exported',len(items),'of',len(raws),'PRs with reviews and checks',flush=True)
(directory/'pilot-snapshot.json').write_text(json.dumps({'items':items,'fetchedAt':stamp},indent=2)+'\n')
print('Snapshot complete:',len(items),'real fork PRs',flush=True)
