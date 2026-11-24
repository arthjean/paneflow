import json, socket, time
W="/var/folders/jz/rpbxwt2d4wjgct6stdnkzd9w0000gn/T/paneflow-ipc-0e043cd22a083973.sock"
V="/var/folders/jz/rpbxwt2d4wjgct6stdnkzd9w0000gn/T/paneflow-serve-0e043cd22a083973.sock"
def call(path,m,p,hello=None):
    s=socket.socket(socket.AF_UNIX); s.connect(path); f=s.makefile("rwb")
    if hello:
        f.write((json.dumps({"jsonrpc":"2.0","method":hello,"params":{"client":"paneflow-cli","protocol":1},"id":0})+"\n").encode()); f.flush(); f.readline()
    f.write((json.dumps({"jsonrpc":"2.0","method":m,"params":p,"id":1})+"\n").encode()); f.flush()
    return json.loads(f.readline())
r=call(V,"workspace.up",{"name":"race-probe-4","layout":"even_h","panes":[{"cwd":"/Users/m1/pfq-cli-flow","name":"race4"}]},hello="worker.hello")
print("worker up:", json.dumps(r)[:250])
sid=r["result"]["surface_ids"][0]; t0=time.time()
for i in range(6):
    rr=call(W,"surface.read",{"surface_id":sid,"lines":500,"fenced":False})
    print(round((time.time()-t0)*1000,1),"ms", json.dumps(rr)[:200])
