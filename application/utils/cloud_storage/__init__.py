from flask import current_app
from google.cloud import storage

class CloudStorage():

    def __init__(self):
        self.storage_client = storage.Client()
        self.bucket_name = current_app.config.get("GCS_BUCKET_NAME")
        self.bucket = self.storage_client.get_bucket(self.bucket_name)
    
    def upload(self, source, destination=None):
        try:
            if not destination:
                destination = source

            blob = self.bucket.blob(destination)
            blob.upload_from_filename(source)
        except Exception as e:
            current_app.logger.info('CloudStorage Upload FAILED: {} to {} | {}'.format(str(source), str(destination), str(e)))

    def download(self, source, destination=None):
        try:
            if not destination:
                destination = source
            
            blob = self.bucket.blob(source)
            blob.download_to_filename(destination)
        except Exception as e:
            current_app.logger.info('CloudStorage Download FAILED: {} to {} | {}'.format(str(source), str(destination), str(e)))
    
    def delete(self, target):
        try:
            blob = self.bucket.blob(target)
            blob.delete()
        except Exception as e:
            current_app.logger.info('CloudStorage Delete FAILED: {} | {}'.format(str(target), str(e)))