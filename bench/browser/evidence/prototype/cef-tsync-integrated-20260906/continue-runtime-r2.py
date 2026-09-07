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
 save()
 stage=json.loads((root/'runtime-bundle/stage.json').read_text())
 env=os.environ.copy();env.update(PANEFLOW_CEF_ROOT=stage['runtime'],CARGO_TARGET_DIR=str(root/'paneflow-target'),CARGO_BUILD_JOBS='4')
 run('build-paneflow-r2',['nice','-n','10','cargo','build','--locked','-p','paneflow-app','-p','paneflow-browser-host','--features','paneflow-browser-host/cef-runtime'],env,workspace)
 for gpu,node,card,vendor,device in [('mesa','renderD128','card1','50_mesa.json','0x164e'),('nvidia','renderD129','card2','10_nvidia.json','0x2705')]:
  run('wayland-'+gpu,['python3',str(repo/'native/browser/experiments/tsync/run-wayland.py'),'--workspace',str(workspace),'--binary',str(root/'paneflow-target/debug/paneflow'),'--host',str(root/'paneflow-target/debug/paneflow-browser-host'),'--render-node','/dev/dri/'+node,'--card','/dev/dri/'+card,'--egl-vendor','/usr/share/glvnd/egl_vendor.d/'+vendor,'--gpui-device',device,'--output',str(repo/('bench/browser/evidence/prototype/cef-tsync-20260906-'+gpu+'-wayland'))],required=False)
 state['status']='FINISHED_REQUIRES_REVIEW';save()
except Exception as error:
 state['status']='FAILED';state['error']=str(error);save();raise
