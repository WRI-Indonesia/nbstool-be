#!/bin/bash

export PYTHONUNBUFFERED=TRUE
printenv

# No `flask db upgrade` here: the shared DB is migrated by the beta branch,
# whose alembic head this legacy checkout doesn't know — running upgrade
# here would crash on the unknown revision.

gunicorn run:app -w 2 --threads 4 -b 0.0.0.0:$PORT --timeout 0 --log-level DEBUG

exec "$@"