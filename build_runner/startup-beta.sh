#!/bin/bash
# Safety net, not the deploy path: any NEW VM (autohealing, scale-up, prod roll)
# boots with the beta container restored. Beta deploys happen via
# cloudbuild-beta.yaml's in-place swap. Runs as root on COS at boot only;
# --restart=always covers mid-life container crashes.
set -e

PROJECT=$(curl -s -H "Metadata-Flavor: Google" "http://metadata.google.internal/computeMetadata/v1/project/project-id")
TOKEN=$(curl -s -H "Metadata-Flavor: Google" "http://metadata.google.internal/computeMetadata/v1/instance/service-accounts/default/token" \
  | sed -n 's/.*"access_token" *: *"\([^"]*\)".*/\1/p')

# COS has no gcloud/jq/python — Secret Manager via REST + sed.
fetch_secret() {
  curl -s -H "Authorization: Bearer $TOKEN" \
    "https://secretmanager.googleapis.com/v1/projects/$PROJECT/secrets/$1/versions/latest:access" \
    | sed -n 's/.*"data" *: *"\([^"]*\)".*/\1/p' | base64 -d
}

# Same env contract as the swap step in cloudbuild-beta.yaml — keep in sync.
cat > /var/lib/app-beta.env <<EOF
PORT=8080
SECRET_KEY=$(fetch_secret PRODUCTION_SECRET_KEY)
CONFIGURATION_SETUP=$(fetch_secret PRODUCTION_CONFIGURATION_SETUP)
DB_SQLALCHEMY_URI=$(fetch_secret PRODUCTION_DB_SQLALCHEMY_URI)
GIS_DB_SQLALCHEMY_URI=$(fetch_secret PRODUCTION_GIS_DB_SQLALCHEMY_URI)
MAIL_BREVO_API_KEY=$(fetch_secret PRODUCTION_MAIL_BREVO_API_KEY)
GCS_BUCKET_NAME=$(fetch_secret PRODUCTION_GCS_BUCKET_NAME)
EOF
chmod 600 /var/lib/app-beta.env

# COS root fs is read-only — root's docker config goes under /var
export DOCKER_CONFIG=/var/lib/app-beta-docker
mkdir -p "$DOCKER_CONFIG"
docker-credential-gcr configure-docker --registries=gcr.io

docker pull "gcr.io/$PROJECT/beta-nbs-service:beta"
docker rm -f app-beta 2>/dev/null || true
docker run -d --name app-beta -p 8081:8080 --restart=always \
  --env-file /var/lib/app-beta.env \
  "gcr.io/$PROJECT/beta-nbs-service:beta"
