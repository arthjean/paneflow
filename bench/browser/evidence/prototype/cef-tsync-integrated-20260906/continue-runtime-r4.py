import datetime,json,os,pathlib,resource,subprocess,time
root=pathlib.Path('/home/arthur/.cache/paneflow-cef-tsync')
repo=pathlib.Path('/home/arthur/dev/paneflow-browser')
workspace=root/'paneflow-snapshot'
resource.setrlimit(resource.RLIMIT_CORE,(0,0))
state=json.loads((root/'runtime-pipeline.json').read_text());state.pop('error',None)
def save():
 (root/'runtime-pipeline.json').write_text(json.dumps(state,indent=2)+'\n')
def run(name,cmd,env=None,cwd=None,required=True):
 phase={'name':name,'command':cmd,'started_at':datetime.datetime.now(datetime.timezone.utc).isoformat()};state['phases'].append(phase);state['status']=name;save()
 with (root/(name+'.log')).open('w') as log:
  p=subprocess.Popen(cmd,env=env,cwd=cwd,stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
  phase['pid']=p.pid;save();p.wait()
 phase['returncode']=p.returncode;phase['finished_at']=datetime.datetime.now(datetime.timezone.utc).isoformat();save()
 if required and p.returncode:raise RuntimeError(name+' failed')
 return p.returncode
try:
 stage=json.loads((root/'runtime-bundle-r2/stage.json').read_text())
 env=os.environ.copy();env.update(PANEFLOW_CEF_ROOT=stage['runtime'],CARGO_TARGET_DIR=str(root/'paneflow-target'),CARGO_BUILD_JOBS='4')
 run('build-paneflow-r4',['nice','-n','10','cargo','build','--locked','-p','paneflow-browser-host','--features','cef-runtime'],env,workspace)
 run('xwayland-nvidia-r3',['python3',str(root/'run-xwayland.py'),'cef-tsync-20260906-nvidia-xwayland-r3'],required=False)
 state['status']='FINISHED_REQUIRES_REVIEW';save()
except Exception as error:
 state['status']='FAILED';state['error']=str(error);save();raise
