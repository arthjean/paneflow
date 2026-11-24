#!/bin/bash
python3 -I ~/pfq-cli-fake/rawrpc2.py host "$PANEFLOW_HOST_ENDPOINT" host.shutdown '{"force":false}' > ~/pfq-cli-fake/inpane-shutdown.out 2>&1
/Users/m1/dev/paneflow/target/debug/paneflow host stop >> ~/pfq-cli-fake/inpane-shutdown.out 2>&1; echo "host stop exit=$?" >> ~/pfq-cli-fake/inpane-shutdown.out
