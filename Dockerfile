# Dockerfile
#
# Two stages. Builder compiles the pip GDAL bindings against libgdal-dev; runtime keeps only
# libgdal + poppler and the finished venv. Base stays ubuntu:24.04 on purpose: noble's apt
# libgdal is 3.8.4, the exact version requirements.txt pins for the GDAL python package, and a
# newer base would break that match.
FROM ubuntu:24.04 AS builder

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
        python3 python3-venv python3-dev \
        build-essential git \
        libgdal-dev \
    && rm -rf /var/lib/apt/lists/*

ENV CPLUS_INCLUDE_PATH=/usr/include/gdal
ENV C_INCLUDE_PATH=/usr/include/gdal

COPY requirements.txt /tmp/requirements.txt
RUN python3 -m venv /opt/venv \
    && /opt/venv/bin/pip install --no-cache-dir --upgrade pip setuptools wheel \
    && /opt/venv/bin/pip install --no-cache-dir -r /tmp/requirements.txt


FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive
# ca-certificates: python's ssl module needs the system trust store to reach the v3 bucket
# (pandas/urllib reads: the CSVs and the species geoparquet). GDAL's /vsicurl carries its own
# CA path, which is why rasters worked while every urllib read died CERTIFICATE_VERIFY_FAILED.
RUN apt-get update && apt-get install -y --no-install-recommends \
        python3 gdal-bin poppler-utils ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /opt/venv /opt/venv
ENV PATH="/opt/venv/bin:$PATH"

# GDAL reading the v3 rasters over /vsicurl: no directory listing per open, cache HTTP block
# reads within a request, retry transient GCS errors instead of failing the component.
# The two cache sizes are deliberately small. VSI_CACHE_SIZE is PER FILE HANDLE and one request
# opens ~30 rasters at once, so 64 MB here would be ~2 GB on a 4 GB box; GDAL_CACHEMAX is the
# per-process block cache, which competes with the float64 AOI windows the components hold.
ENV GDAL_DISABLE_READDIR_ON_OPEN=EMPTY_DIR \
    VSI_CACHE=TRUE \
    VSI_CACHE_SIZE=16777216 \
    GDAL_CACHEMAX=256 \
    GDAL_HTTP_MAX_RETRY=3 \
    GDAL_HTTP_RETRY_DELAY=1

WORKDIR /app
COPY . /app
RUN chmod +x /app/build_runner/entrypoint.sh

ENTRYPOINT ["/app/build_runner/entrypoint.sh"]
