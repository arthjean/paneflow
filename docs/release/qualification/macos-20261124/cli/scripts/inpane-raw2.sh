#!/bin/bash
V=${PANEFLOW_SOCKET_PATH/paneflow-ipc-/paneflow-serve-}
python3 -I ~/pfq-cli-fake/rawrpc2.py worker "$V" layout.launch '{"surface_id":999999,"session":"00000000-0000-0000-0000-000000000002"}' > ~/pfq-cli-fake/inpane-raw2.out 2>&1
