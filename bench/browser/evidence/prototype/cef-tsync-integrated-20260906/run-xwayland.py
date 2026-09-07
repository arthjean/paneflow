import json,os,pathlib,resource,subprocess,sys
root=pathlib.Path('/home/arthur/.cache/paneflow-cef-tsync')
repo=pathlib.Path('/home/arthur/dev/paneflow-browser')
output=repo/'bench/browser/evidence/prototype'/sys.argv[1]
output.mkdir(exist_ok=False)
resource.setrlimit(resource.RLIMIT_CORE,(0,0))
env=os.environ.copy();env.pop('WAYLAND_DISPLAY',None)
env.update(ZED_DEVICE_ID='0x2705',PANEFLOW_BROWSER_RENDER_NODE='/dev/dri/renderD129',__EGL_VENDOR_LIBRARY_FILENAMES='/usr/share/glvnd/egl_vendor.d/10_nvidia.json')
command=['bun','scripts/browser-qualification/prototype.mjs','--binary',str(root/'paneflow-target/debug/paneflow'),'--host',str(root/'paneflow-target/debug/paneflow-browser-host'),'--display','x11','--scenario','empty','--steps','input,resize,scale,host-loss','--hold','1','--output',str(output/'prototype')]
receipt={'command':command,'display':env.get('DISPLAY'),'session_type':env.get('XDG_SESSION_TYPE'),'native_xorg_qualification':False}
(output/'command.json').write_text(json.dumps(receipt,indent=2)+'\n')
with (output/'runner.stdout').open('w') as stdout,(output/'runner.stderr').open('w') as stderr:
 result=subprocess.run(command,cwd=root/'paneflow-snapshot',env=env,stdout=stdout,stderr=stderr,timeout=320)
receipt['returncode']=result.returncode
(output/'command.json').write_text(json.dumps(receipt,indent=2)+'\n')
print(json.dumps(receipt,indent=2))
sys.exit(result.returncode)
