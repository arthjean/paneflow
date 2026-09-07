import datetime,json,os,pathlib,shutil,signal,subprocess,sys,time
root=pathlib.Path('/home/arthur/.cache/paneflow-cef-tsync')
src=root/'work/chromium/src'
phase=sys.argv[1]
env=os.environ.copy();env['PATH']=str(root/'work/depot_tools')+os.pathsep+env['PATH'];env['DEPOT_TOOLS_UPDATE']='0'
command=['nice','-n','10','autoninja','-j',env.get('CEF_BUILD_JOBS','6'),'-l',env.get('CEF_BUILD_LOAD','10'),'-C','out/Release_GN_x64',*sys.argv[2:]]
receipt={'started_at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'command':command,'cwd':str(src)}
with (root/(phase+'.log')).open('w') as log:
 process=subprocess.Popen(command,cwd=src,env=env,stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
 receipt['pid']=process.pid
 (root/(phase+'.json')).write_text(json.dumps(receipt,indent=2)+'\n')
 while process.poll() is None:
  if shutil.disk_usage(src).free < 25*1024**3:
   receipt['stopped_reason']='less than 25 GiB disk free';os.killpg(process.pid,signal.SIGTERM);break
  available=int(next(line.split()[1] for line in pathlib.Path('/proc/meminfo').read_text().splitlines() if line.startswith('MemAvailable:')))*1024
  if available < 5*1024**3:
   receipt['stopped_reason']='less than 5 GiB memory available';os.killpg(process.pid,signal.SIGINT);break
  time.sleep(2)
 try:process.wait(timeout=30)
 except subprocess.TimeoutExpired:os.killpg(process.pid,signal.SIGKILL);process.wait()
 receipt['returncode']=process.returncode
 receipt['finished_at']=datetime.datetime.now(datetime.timezone.utc).isoformat()
 (root/(phase+'.json')).write_text(json.dumps(receipt,indent=2)+'\n')
 print(json.dumps(receipt,indent=2))
 sys.exit(process.returncode or 0)
