#!/bin/bash

export PYTHONUNBUFFERED=TRUE
printenv

flask db upgrade

gunicorn run:app -w 2 --threads 4 -b 0.0.0.0:$PORT --timeout 0 --log-level DEBUG

exec "$@"