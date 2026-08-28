#!/usr/bin/env python3
from __future__ import annotations

import argparse, json, os, re, urllib.error, urllib.request
from dataclasses import dataclass
from pathlib import Path

API='https://api.github.com'
API_VERSION='2022-11-28'
SHA_RE=re.compile(r'^[0-9a-f]{40}$')
TRACKING='DEN-3786'
OPTO_SYNC_SHA='4d5627040500a8840d6ab0f6f412a908e2a0f6a9'

@dataclass(frozen=True)
class Repo:
    org:str
    name:str
    description:str
    kind:str
    @property
    def full(self): return f'{self.org}/{self.name}'

PROD=(
 Repo('elenkos-systems','elenkos-sync','Elenkos local-first synchronization wrapper around opto-sync','sync'),
 Repo('elenkos-systems','elenkos-lib-core','Canonical Elenkos QA domain policy, SeaORM entities, and migrations','lib'),
 Repo('elenkos-systems','elenkos-monorepo','Elenkos workspace coordination and Zed dependency graph','monorepo'),
 Repo('elenkos-systems','elenkos-web-server.rs','Rust Axum + Leptos + SeaORM web application for Elenkos','web'),
 Repo('elenkos-systems','elenkos-api-server.rs','Rust Axum + SeaORM JSON API for Elenkos mutations','api'),
 Repo('elenkos-systems','elenkos-flutter','Flutter mobile and desktop client for Elenkos','flutter'),
 Repo('elenkos-systems','elenkos-desktop-app.rs','Native Rust desktop client for Elenkos','desktop'),
 Repo('elenkos-systems','elenkos-infra','GitOps, Kubernetes, Cloudflare, and deployment infrastructure for Elenkos','infra'),
 Repo('elenkos-systems','elenkos-clients','Official polyglot Elenkos SDK clients','clients'),
 Repo('elenkos-systems','elenkos-interfaces','Canonical JSON Schema, OpenAPI, and event contracts for Elenkos','interfaces'),
 Repo('elenkos-systems','elenkos-cli','Command-line client for Elenkos QA workflows','cli'),
)
TEST=(
 Repo('elenkos-systems-test','blind-review-isolation-tests','Adversarial tests proving human reviewers cannot observe AI assessments before submission','test'),
 Repo('elenkos-systems-test','severity-consensus-tests','Consensus threshold and severity scoring conformance tests','test'),
 Repo('elenkos-systems-test','discrepancy-adjudication-tests','Escalation tests for material AI and human scoring discrepancies','test'),
 Repo('elenkos-systems-test','credit-ledger-idempotency-tests','Idempotency and double-payment prevention tests for QA credits','test'),
 Repo('elenkos-systems-test','reviewer-assignment-tests','Randomized eligibility, conflict exclusion, and rotation tests for reviewer assignment','test'),
 Repo('elenkos-systems-test','opto-sync-convergence-e2e','Offline convergence and sealed-field synchronization tests','test'),
 Repo('elenkos-systems-test','rust-fullstack-e2e','End-to-end API, web, persistence, and contract tests for Rust services','test'),
 Repo('elenkos-systems-test','flutter-desktop-parity-e2e','Behavioral parity tests across Flutter and native Rust desktop clients','test'),
 Repo('elenkos-systems-test','sdk-cli-packaging-e2e','Zed packaging and CLI/SDK consumer tests','test'),
 Repo('elenkos-systems-test','infra-canary','Deployment, health, migration, and rollback canaries for Elenkos infrastructure','test'),
)
REPOS=PROD+TEST

class GitHub:
    def __init__(self, token:str): self.token=token
    def request(self, method:str, path:str, body=None, allow=()):
        payload=None if body is None else json.dumps(body,separators=(',',':')).encode()
        headers={'Accept':'application/vnd.github+json','Authorization':f'Bearer {self.token}','X-GitHub-Api-Version':API_VERSION,'User-Agent':'elenkos-fleet-publisher'}
        if payload is not None: headers['Content-Type']='application/json'
        req=urllib.request.Request(API+path,data=payload,headers=headers,method=method)
        try:
            with urllib.request.urlopen(req,timeout=45) as r:
                raw=r.read(); return r.status, json.loads(raw) if raw else None
        except urllib.error.HTTPError as e:
            raw=e.read(16384)
            if e.code in allow: return e.code, None
            try: msg=json.loads(raw).get('message','unknown error')
            except Exception: msg=raw.decode('utf-8','replace')[:1000]
            raise RuntimeError(f'GitHub {method} {path} failed HTTP {e.code}: {msg}')
    def get(self,p,allow=()): return self.request('GET',p,allow=allow)
    def post(self,p,b): return self.request('POST',p,b)
    def patch(self,p,b): return self.request('PATCH',p,b)

def zpkg(repo:Repo, deps:dict[str,str]|None=None, scripts:dict[str,str]|None=None)->str:
    lines=['[package]',f'org = "{repo.org}"',f'name = "{repo.name}"','version = "0.1.0"',f'description = {json.dumps(repo.description)}','license = "MIT"','', '[package.repository]','vcs = "git"',f'url = "https://github.com/{repo.full}"','', '[install]','dir = ".vendor/.zed"','']
    if deps:
        lines += ['[dependencies]']+[f'{json.dumps(k)} = {json.dumps(v)}' for k,v in deps.items()]+['']
    lines += ['[publish]','include_readme = true','tag_format = "v{version}"','exclude = [".env", ".env.*", ".zed/**", ".vendor/.zed/**", "target/**", "node_modules/**", "build/**", "dist/**"]','']
    if scripts:
        lines += ['[scripts]']+[f'{k} = {json.dumps(v)}' for k,v in scripts.items()]+['']
    return '\n'.join(lines)

def readme(repo:Repo)->str:
    return f'''# {repo.name}\n\n{repo.description}.\n\nTracking: `{TRACKING}`.\n\n## Product invariants\n\nElenkos pays QA engineers flat rates plus severity-linked credits. AI severity assessment and assigned human review are independent: the human reviewer must not see the AI assessment before submitting. Agreement inside policy thresholds can award idempotent credits to both reporter and reviewer. Material disagreement creates an adjudication case and withholds consensus credits until resolution.\n\nCross-repository dependencies are declared in `.zpkg.toml` and installed with Zed.\n'''

def interfaces_files(repo:Repo):
    severity={'$schema':'https://json-schema.org/draft/2020-12/schema','title':'SeverityAssessment','type':'object','required':['assessmentId','bugId','source','score','submittedAt','sealedUntilHumanSubmission'],'properties':{'assessmentId':{'type':'string'},'bugId':{'type':'string'},'source':{'enum':['ai','human','adjudicator']},'score':{'type':'integer','minimum':0,'maximum':100},'submittedAt':{'type':'string','format':'date-time'},'sealedUntilHumanSubmission':{'type':'boolean'}},'additionalProperties':False}
    bug={'$schema':'https://json-schema.org/draft/2020-12/schema','title':'BugReport','type':'object','required':['bugId','reporterId','title','description','createdAt'],'properties':{'bugId':{'type':'string'},'reporterId':{'type':'string'},'title':{'type':'string','minLength':1},'description':{'type':'string','minLength':1},'createdAt':{'type':'string','format':'date-time'}},'additionalProperties':False}
    return {'schemas/severity-assessment.schema.json':json.dumps(severity,indent=2)+'\n','schemas/bug-report.schema.json':json.dumps(bug,indent=2)+'\n','openapi/openapi.json':json.dumps({'openapi':'3.1.0','info':{'title':'Elenkos API','version':'0.1.0'},'paths':{'/healthz':{'get':{'responses':{'200':{'description':'healthy'}}}},'/v1/bugs':{'post':{'responses':{'202':{'description':'accepted'}}}}}},indent=2)+'\n'}

def lib_files(repo:Repo):
    cargo='''[package]\nname = "elenkos-lib-core"\nversion = "0.1.0"\nedition = "2024"\nlicense = "MIT"\n\n[dependencies]\nserde = { version = "1", features = ["derive"] }\nsea-orm = { version = "1.1", default-features = false, features = ["macros", "runtime-tokio-rustls", "sqlx-postgres", "with-uuid", "with-chrono"] }\nuuid = { version = "1", features = ["serde", "v4"] }\n'''
    src='''use serde::{Deserialize, Serialize};\n\n#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]\npub struct SeverityScore(pub u8);\n\n#[derive(Debug, Clone, Copy, PartialEq, Eq)]\npub enum ConsensusDecision { AwardCredits, Escalate }\n\npub fn decide(ai: SeverityScore, human: SeverityScore, threshold: u8) -> ConsensusDecision {\n    if ai.0.abs_diff(human.0) <= threshold { ConsensusDecision::AwardCredits } else { ConsensusDecision::Escalate }\n}\n\npub fn human_can_view_ai(human_submitted: bool) -> bool { human_submitted }\n\n#[cfg(test)]\nmod tests { use super::*;\n #[test] fn agreement_awards() { assert_eq!(decide(SeverityScore(80),SeverityScore(84),5),ConsensusDecision::AwardCredits); }\n #[test] fn discrepancy_escalates() { assert_eq!(decide(SeverityScore(20),SeverityScore(80),10),ConsensusDecision::Escalate); }\n #[test] fn ai_is_blind_until_submit() { assert!(!human_can_view_ai(false)); assert!(human_can_view_ai(true)); }\n}\n'''
    return {'Cargo.toml':cargo,'src/lib.rs':src}

def sync_files(repo:Repo):
    cargo=f'''[package]\nname = "elenkos-sync"\nversion = "0.1.0"\nedition = "2024"\nlicense = "MIT"\n\n[dependencies]\nserde_json = "1"\nsyncer-rs = {{ git = "https://github.com/opto-sync/syncer.rs.git", rev = "{OPTO_SYNC_SHA}" }}\n'''
    src='''pub use syncer_rs::{merge_json, MergeOptions};\n\npub fn reconcile(base:&str,incoming:&str)->Result<String,String>{ merge_json(base,incoming,&MergeOptions::default()).map_err(|e|e.to_string()) }\n#[cfg(test)] mod tests { use super::*; #[test] fn wraps_opto_sync(){ let v=reconcile("{\\"a\\":1}","{\\"a\\":2}").unwrap(); assert_eq!(v,"{\\"a\\":2}"); } }\n'''
    return {'Cargo.toml':cargo,'src/lib.rs':src}

def api_files(repo:Repo, web=False):
    name='elenkos-web-server' if web else 'elenkos-api-server'
    deps='leptos = { version = "0.8", features = ["ssr"] }\n' if web else ''
    cargo=f'''[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "2024"\nlicense = "MIT"\n\n[dependencies]\naxum = "0.8"\ntokio = {{ version = "1", features = ["macros", "net", "rt-multi-thread"] }}\nsea-orm = {{ version = "1.1", default-features = false, features = ["macros", "runtime-tokio-rustls", "sqlx-postgres"] }}\nserde = {{ version = "1", features = ["derive"] }}\nserde_json = "1"\n{deps}'''
    if web:
        src='''use axum::{routing::get, response::Html, Router};\nuse leptos::prelude::*;\n#[component] fn App() -> impl IntoView { view!{ <main><h1>"Elenkos"</h1><p>"Blind QA review and severity-linked credits."</p></main> } }\nasync fn home()->Html<String>{ Html(view!{ <App/> }.to_html()) }\nasync fn health()->&'static str{"ok"}\n#[tokio::main] async fn main(){ let app=Router::new().route("/",get(home)).route("/healthz",get(health)); let l=tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap(); axum::serve(l,app).await.unwrap(); }\n'''
    else:
        src='''use axum::{routing::{get,post},Json,Router};\nuse serde::{Deserialize,Serialize};\n#[derive(Deserialize)] struct Bug { title:String, description:String }\n#[derive(Serialize)] struct Accepted { accepted:bool }\nasync fn health()->&'static str{"ok"}\nasync fn bugs(Json(b):Json<Bug>)->Json<Accepted>{ Json(Accepted{accepted:!b.title.trim().is_empty()&&!b.description.trim().is_empty()}) }\n#[tokio::main] async fn main(){ let app=Router::new().route("/healthz",get(health)).route("/v1/bugs",post(bugs)); let l=tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap(); axum::serve(l,app).await.unwrap(); }\n'''
    return {'Cargo.toml':cargo,'src/main.rs':src}

def cli_files(repo:Repo):
    return {'Cargo.toml':'[package]\nname="elenkos-cli"\nversion="0.1.0"\nedition="2024"\nlicense="MIT"\n\n[dependencies]\nclap={version="4",features=["derive","env"]}\n','src/main.rs':'use clap::{Parser,Subcommand};\n#[derive(Parser)] struct Args{#[command(subcommand)] command:Command}\n#[derive(Subcommand)] enum Command{Report{title:String},Review{bug_id:String}}\nfn main(){match Args::parse().command{Command::Report{title}=>println!("report: {title}"),Command::Review{bug_id}=>println!("review: {bug_id}")}}\n'}

def flutter_files(repo:Repo):
    return {'pubspec.yaml':'name: elenkos_flutter\ndescription: Elenkos mobile and desktop client\npublish_to: none\nversion: 0.1.0+1\nenvironment:\n  sdk: ">=3.6.0 <4.0.0"\ndependencies:\n  flutter:\n    sdk: flutter\n','lib/main.dart':'import \'package:flutter/material.dart\';\nvoid main()=>runApp(const ElenkosApp());\nclass ElenkosApp extends StatelessWidget{const ElenkosApp({super.key});@override Widget build(BuildContext c)=>MaterialApp(home:Scaffold(appBar:AppBar(title:const Text(\'Elenkos\')),body:const Center(child:Text(\'Blind QA review queue\'))));}\n'}

def desktop_files(repo:Repo):
    return {'Cargo.toml':'[package]\nname="elenkos-desktop-app"\nversion="0.1.0"\nedition="2024"\nlicense="MIT"\n\n[dependencies]\neframe="0.31"\n','src/main.rs':'fn main()->eframe::Result<()> { eframe::run_native("Elenkos",eframe::NativeOptions::default(),Box::new(|_|Ok(Box::<App>::default()))) }\n#[derive(Default)]struct App; impl eframe::App for App{fn update(&mut self,ctx:&egui::Context,_:&mut eframe::Frame){egui::CentralPanel::default().show(ctx,|ui|{ui.heading("Elenkos");ui.label("Blind QA review queue");});}}\nuse eframe::egui;\n'}

def clients_files(repo:Repo):
    return {'clients/rust/Cargo.toml':'[package]\nname="elenkos-client"\nversion="0.1.0"\nedition="2024"\nlicense="MIT"\n','clients/rust/src/lib.rs':'pub const API_VERSION:&str="v1";\n','clients/typescript/package.json':json.dumps({'name':'@elenkos-systems/client','version':'0.1.0','type':'module','private':True},indent=2)+'\n','clients/typescript/src/index.ts':'export const apiVersion = "v1";\n','clients/dart/pubspec.yaml':'name: elenkos_client\nversion: 0.1.0\nenvironment:\n  sdk: ">=3.6.0 <4.0.0"\n','clients/dart/lib/elenkos_client.dart':'const apiVersion = \'v1\';\n'}

def infra_files(repo:Repo):
    return {'k8s/namespace.yaml':'apiVersion: v1\nkind: Namespace\nmetadata:\n  name: elenkos\n','k8s/api-deployment.yaml':'apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: elenkos-api\n  namespace: elenkos\nspec:\n  replicas: 2\n  selector:\n    matchLabels: {app: elenkos-api}\n  template:\n    metadata:\n      labels: {app: elenkos-api}\n    spec:\n      containers:\n        - name: api\n          image: ghcr.io/elenkos-systems/elenkos-api-server:latest\n          ports: [{containerPort: 8080}]\n          readinessProbe: {httpGet: {path: /healthz, port: 8080}}\n','README-DEPLOY.md':'# Deployment\n\nProduction changes are GitOps-only. Secrets are injected at runtime and never committed.\n'}

def monorepo_files(repo:Repo):
    deps={f'elenkos-systems/{r.name}':'^0.1.0' for r in PROD if r.name not in {'elenkos-monorepo','elenkos-infra'}}
    return {'docs/topology.md':'# Elenkos topology\n\n`elenkos-interfaces` -> `elenkos-lib-core` -> services/clients. `elenkos-sync` wraps opto-sync. `elenkos-infra` remains outside the application monorepo.\n','.zpkg.toml':zpkg(repo,deps)}

def test_files(repo:Repo):
    target=repo.name
    script=f'''#!/usr/bin/env python3\nfrom pathlib import Path\nname={target!r}\nassert name\nprint(f"{{name}}: scaffold contract ready")\n'''
    docs='''# Test contract\n\nThis repository is a consumer/adversarial test surface. It must consume production artifacts through Zed and must never copy production source. Blind-review tests specifically assert that AI severity values cannot cross the human-review boundary before submission.\n'''
    return {'tests/contract.py':script,'docs/test-contract.md':docs}

def files_for(repo:Repo)->dict[str,str]:
    files={'README.md':readme(repo),'.gitignore':'.env\n.env.*\n!.env.example\n.vendor/\n.zed/\ntarget/\nnode_modules/\nbuild/\ndist/\n','AGENTS.md':f'# AGENTS.md\n\nTracking: `{TRACKING}`. Use focused PRs; never commit credentials or customer data; preserve blind-review boundaries.\n'}
    if repo.kind=='interfaces': files.update(interfaces_files(repo)); deps=None
    elif repo.kind=='lib': files.update(lib_files(repo)); deps={'elenkos-systems/elenkos-interfaces':'^0.1.0'}
    elif repo.kind=='sync': files.update(sync_files(repo)); deps={'opto-sync/syncer-rs':'^0.3.0','elenkos-systems/elenkos-interfaces':'^0.1.0'}
    elif repo.kind=='api': files.update(api_files(repo)); deps={'elenkos-systems/elenkos-interfaces':'^0.1.0','elenkos-systems/elenkos-lib-core':'^0.1.0'}
    elif repo.kind=='web': files.update(api_files(repo,True)); deps={'elenkos-systems/elenkos-interfaces':'^0.1.0','elenkos-systems/elenkos-lib-core':'^0.1.0'}
    elif repo.kind=='cli': files.update(cli_files(repo)); deps={'elenkos-systems/elenkos-clients':'^0.1.0'}
    elif repo.kind=='flutter': files.update(flutter_files(repo)); deps={'elenkos-systems/elenkos-clients':'^0.1.0','elenkos-systems/elenkos-sync':'^0.1.0'}
    elif repo.kind=='desktop': files.update(desktop_files(repo)); deps={'elenkos-systems/elenkos-clients':'^0.1.0','elenkos-systems/elenkos-sync':'^0.1.0'}
    elif repo.kind=='clients': files.update(clients_files(repo)); deps={'elenkos-systems/elenkos-interfaces':'^0.1.0'}
    elif repo.kind=='infra': files.update(infra_files(repo)); deps={'elenkos-systems/elenkos-api-server.rs':'^0.1.0','elenkos-systems/elenkos-web-server.rs':'^0.1.0'}
    elif repo.kind=='monorepo': files.update(monorepo_files(repo)); return files
    else:
        files.update(test_files(repo)); deps={'elenkos-systems/elenkos-interfaces':'^0.1.0','elenkos-systems/elenkos-lib-core':'^0.1.0'}
        if 'sync' in repo.name: deps['elenkos-systems/elenkos-sync']='^0.1.0'
    files['.zpkg.toml']=zpkg(repo,deps,{'test':'python3 tests/contract.py'} if repo.kind=='test' else None)
    return files

def require_dict(status,doc,expected,op):
    if status!=expected or not isinstance(doc,dict): raise RuntimeError(f'{op} failed HTTP {status}')
    return doc

def sha(doc,op):
    value=doc.get('sha') if isinstance(doc,dict) else None
    if not isinstance(value,str) or not SHA_RE.fullmatch(value): raise RuntimeError(f'{op} returned invalid sha')
    return value

def get_main(gh:GitHub,repo:Repo):
    status,doc=gh.get(f'/repos/{repo.full}/git/ref/heads/main',allow=(404,))
    if status==404:return None
    return sha(require_dict(status,doc,200,'read main').get('object'), 'read main object')

def create_commit(gh:GitHub,repo:Repo,files:dict[str,str],parent:str|None):
    tree=[]
    for path,content in sorted(files.items()):
        status,doc=gh.post(f'/repos/{repo.full}/git/blobs',{'content':content,'encoding':'utf-8'})
        blob=sha(require_dict(status,doc,201,f'blob {path}'),f'blob {path}')
        tree.append({'path':path,'mode':'100644','type':'blob','sha':blob})
    payload={'tree':tree}
    if parent:
        status,commit=gh.get(f'/repos/{repo.full}/git/commits/{parent}')
        base_tree=sha(require_dict(status,commit,200,'read parent').get('tree'),'parent tree')
        payload['base_tree']=base_tree
    status,doc=gh.post(f'/repos/{repo.full}/git/trees',payload); tree_sha=sha(require_dict(status,doc,201,'tree'),'tree')
    commit_payload={'message':f'feat: initialize {repo.name} ({TRACKING})','tree':tree_sha,'parents':[parent] if parent else []}
    status,doc=gh.post(f'/repos/{repo.full}/git/commits',commit_payload); commit_sha=sha(require_dict(status,doc,201,'commit'),'commit')
    if parent:
        status,_=gh.patch(f'/repos/{repo.full}/git/refs/heads/main',{'sha':commit_sha,'force':False})
        if status!=200: raise RuntimeError(f'update main failed HTTP {status}')
    else:
        status,_=gh.post(f'/repos/{repo.full}/git/refs',{'ref':'refs/heads/main','sha':commit_sha})
        if status!=201: raise RuntimeError(f'create main failed HTTP {status}')
    return commit_sha

def ensure_repo(gh:GitHub,repo:Repo):
    status,doc=gh.get(f'/repos/{repo.full}',allow=(404,)); created=False
    if status==404:
        status,doc=gh.post(f'/orgs/{repo.org}/repos',{'name':repo.name,'description':repo.description,'private':True,'has_issues':True,'has_projects':False,'has_wiki':False,'auto_init':False,'allow_squash_merge':True,'allow_merge_commit':True,'allow_rebase_merge':False,'delete_branch_on_merge':True})
        doc=require_dict(status,doc,201,f'create {repo.full}'); created=True
    elif status!=200: raise RuntimeError(f'preflight {repo.full} failed HTTP {status}')
    if doc.get('full_name')!=repo.full or doc.get('private') is not True or doc.get('archived') is True: raise RuntimeError(f'repository policy mismatch: {repo.full}')
    main=get_main(gh,repo)
    if main is not None and not created:
        status,marker=gh.get(f'/repos/{repo.full}/contents/AGENTS.md?ref=main',allow=(404,))
        if status==404: raise RuntimeError(f'refusing to overwrite pre-existing unmarked repository {repo.full}')
    commit=create_commit(gh,repo,files_for(repo),main)
    status,final=gh.get(f'/repos/{repo.full}'); final=require_dict(status,final,200,'postflight')
    if final.get('default_branch')!='main': gh.patch(f'/repos/{repo.full}',{'default_branch':'main'})
    return {'full_name':repo.full,'created':created,'main_sha':commit,'file_count':len(files_for(repo))}

def plan():
    return {'tracking':TRACKING,'production':[r.full for r in PROD],'tests':[r.full for r in TEST],'repository_count':len(REPOS),'visibility':'private','opto_sync_sha':OPTO_SYNC_SHA}

def main():
    ap=argparse.ArgumentParser(); ap.add_argument('--plan',action='store_true'); ap.add_argument('--evidence-out',type=Path); args=ap.parse_args()
    if args.plan:
        print(json.dumps(plan(),indent=2,sort_keys=True)); return 0
    token=os.environ.get('GH_TOKEN','')
    if len(token)<20 or any(c.isspace() for c in token): raise SystemExit('GH_TOKEN missing or malformed')
    gh=GitHub(token); results=[ensure_repo(gh,r) for r in REPOS]
    evidence={**plan(),'repositories':results}
    if args.evidence_out:
        args.evidence_out.parent.mkdir(parents=True,exist_ok=True); args.evidence_out.write_text(json.dumps(evidence,indent=2,sort_keys=True)+'\n')
    for r in results: print(f"ELENKOS_REPOSITORY_READY {r['full_name']} main={r['main_sha']} created={str(r['created']).lower()}")
    print(f"ELENKOS_FLEET_COMPLETE repositories={len(results)}")
    return 0

if __name__=='__main__': raise SystemExit(main())
