# build_runner/config_builder.py
import os
import sys
import uuid

try:
    from dotenv import load_dotenv
    dotenv_path = os.path.join(os.path.dirname(__file__), '../.env')
    if os.path.exists(dotenv_path):
        load_dotenv(dotenv_path)
except Exception as e:
    print('error load dotenv: {}'.format(str(e)))


def write_cloudbuild_yaml(environment, sa_mail):
    raw = '''
# cloudbuild.yaml
steps:
- name: gcr.io/cloud-builders/docker
  args:
    - build
    - '-t'
    - 'gcr.io/$PROJECT_ID/{environment}-nbs-service:{environment}'
    - '.'
- name: gcr.io/cloud-builders/docker
  args:
    - push
    - 'gcr.io/$PROJECT_ID/{environment}-nbs-service:{environment}'
- name: gcr.io/google.com/cloudsdktool/cloud-sdk
  entrypoint: bash
  args: [
    '-c',
    'gcloud compute instance-templates create-with-container {environment}-be-nbs-service-{uuid}
    --container-image gcr.io/$PROJECT_ID/{environment}-nbs-service:{environment}
    --region asia-southeast1 --machine-type {machine_type}
    --boot-disk-type pd-balanced --boot-disk-size 60GB
    --tags http-server,https-server,lb-health-check
    --shielded-integrity-monitoring --shielded-vtpm
    --scopes default,storage-full,cloud-platform,https://www.googleapis.com/auth/drive
    --service-account {service_account}
    --metadata-from-file startup-script=build_runner/startup-beta.sh
    --container-env PORT={port},{env_args}
    '
  ]
  secretEnv: [{envs}]
- name: gcr.io/cloud-builders/gcloud
  args:
    - compute
    - 'instance-groups'
    - managed
    - 'rolling-action'
    - 'start-update'
    - '{environment}-be-nbs-service'
    - '--replacement-method'
    - substitute
    - '--version'
    - 'template={environment}-be-nbs-service-{uuid}'
    - '--zone'
    - 'asia-southeast1-b'
    - '--max-unavailable'
    - '50%'

availableSecrets:
  secretManager:
{secret_env}

timeout: 1600s'''

    arg    = "    - '--container-env'"
    val    = "    - '{}=$${}'"
    secret = "    - versionName: projects/$PROJECT_NUMBER/secrets/{}_{}/versions/latest"
    env    = "      env: '{}_{}'"

    mapping_vars_to_secret_gcp = [
        n
        for n 
        in os.environ.get('{}_MAPPING_VARS_TO_SECRET_GCP'.format(environment)).split(';')
    ]

    args    = []
    secrets = []
    envs    = []
    env_args = []
    for var in mapping_vars_to_secret_gcp:
        args.append(arg)
        args.append(
            val.format(
                var,
                '{}_{}'.format(environment, var)
            )
        )

        env_args.append(val.replace('    - ', '').replace("'", '').format(
            var,
            '{}_{}'.format(environment, var)
        ))
        
        secrets.append(secret.format(environment, var))
        secrets.append(env.format(environment, var))

        envs.append("'{}_{}'".format(environment, var))

    open('build_runner/cloudbuild.yaml', 'w').write(raw.format(
        environment = environment.lower(),
        build_args = '\n'.join(args),
        secret_env = '\n'.join(secrets),
        envs = ', '.join(envs),
        gcs_volume = os.environ.get('{}_GCS_VOLUME_NAME'.format(environment)),
        port = os.environ.get('PORT'),
        env_args = ','.join(env_args),
        uuid = str(uuid.uuid4()),
        service_account = sa_mail,
        machine_type = os.environ.get('{}_MACHINE_TYPE'.format(environment))
    ))
 
    print('INFO: cloudbuild.yaml successfully generated.')
    print('INFO: {}'.format(open('build_runner/cloudbuild.yaml', 'r').read()))

def write_gcp_sa_json(environment):
    import base64
    import json

    encoded = os.environ.get('{}_GCP_SERVICE_ACCOUNT'.format(environment)).encode('utf8')
    decoded = base64.b64decode(encoded)
    sa_mail = json.loads(decoded).get('client_email')
    
    open('gcp-sa.json', 'wb').write(decoded)

    print('INFO: gcp-sa.json successfully generated.')
    print('INFO: {}'.format(open('gcp-sa.json', 'r').read()[:10]))

    return sa_mail

if __name__ == '__main__':
    environment = sys.argv[1].upper()
    print(environment)
    sa_mail = write_gcp_sa_json(environment)
    write_cloudbuild_yaml(environment, sa_mail)