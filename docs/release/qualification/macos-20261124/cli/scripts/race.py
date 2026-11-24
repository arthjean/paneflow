import json, socket, sys, time
P="/var/folders/jz/rpbxwt2d4wjgct6stdnkzd9w0000gn/T/paneflow-ipc-0e043cd22a083973.sock"
def call(m,p):
    s=socket.socket(socket.AF_UNIX); s.connect(P); f=s.makefile("rwb")
    f.write((json.dumps({"jsonrpc":"2.0","method":m,"params":p,"id":1})+"\n").encode()); f.flush()
    return json.loads(f.readline())
r=call("workspace.up",{"name":"race-probe","layout":"even_h","panes":[{"cwd":"/Users/m1/pfq-cli-flow","name":"race1"}]})
print("up:", json.dumps(r)[:300])
sid=r["result"]["surface_ids"][0]
t0=time.time()
for i in range(8):
    rr=call("surface.read",{"surface_id":sid,"lines":500,"fenced":False})
    print(round(time.time()-t0,3), json.dumps(rr)[:200])
    time.sleep(0.15)
