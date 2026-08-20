from googleapiclient.discovery import build
from google.oauth2 import service_account
from googleapiclient.http import MediaFileUpload

class CloudDrive():

    def __init__(self):
        # self.creds = service_account.Credentials.from_service_account_file('/home/del/Downloads/gdrive-admin-staging-scene-428902-53e44477e790.json', scopes=['https://www.googleapis.com/auth/drive'])
        self.creds = None
        self.service = build('drive', 'v3', credentials=self.creds)

    
    def create_folder(self, name:str, parents_id:str) -> str:
        folder_metadata = {
            'name': name,
            'mimeType': 'application/vnd.google-apps.folder',
            'parents': [parents_id]
        }
        folder = self.service.files().create(body=folder_metadata, fields="id,webViewLink").execute()
        return folder
    

    def create_document(self, path:str, name:str, parents_id:str) -> str: # docx only
        file_metadata = {
            "name": name,
            "parents": [parents_id],
        }
        media = MediaFileUpload(
            path,
            mimetype="application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            resumable=True
        )
        file = self.service.files().create(body=file_metadata, media_body=media, fields='id').execute()
        return file.get('id')
    

    def create_anyone_permission(self, file_id:str) -> str:
        permission_metadata = {
            'type': 'anyone',
            'role': 'writer',
        }
        permission = self.service.permissions().create(fileId=file_id, body=permission_metadata).execute()
        return permission
    
    
    def get_file_link(self, file_id:str) -> str:
        file = self.service.files().get(fileId=file_id, fields='webViewLink').execute()
        return file.get('webViewLink')
    

    def create_anyone_folder(self, name:str, parents_id:str):
        folder = self.create_folder(name, parents_id)
        self.create_anyone_permission(folder.get('id'))
        return folder