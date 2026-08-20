#!/bin/bash

export PYTHONUNBUFFERED=TRUE
printenv

flask db upgrade

# c2d-highcpu-2 (2 vCPU, 4 GB): one worker halves the geo-stack baseline RSS, and the heavy work
# releases the GIL in GDAL/numpy C code so 8 threads still use both cores. MIG provides the
# process redundancy. Timeout 300 is worker-hang protection, not a request limit: the gthread
# worker keeps heartbeating while long NDJSON streams run.
gunicorn run:app -w 1 --threads 8 -b 0.0.0.0:$PORT --timeout 300 --graceful-timeout 300 --keep-alive 620 --worker-tmp-dir /dev/shm --log-level INFO

exec "$@"