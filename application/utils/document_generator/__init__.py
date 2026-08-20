import sqlalchemy as db
from sqlalchemy.orm import sessionmaker
from sqlalchemy.sql import text

import os
import csv
import uuid
import psycopg2

from datetime import datetime, timedelta
# from celery import Celery
from shapely.geometry import Polygon, MultiPolygon

from config import Config
from .docx_template import fill_template_variables, fill_form_template_variables

# from application.utils.mail import BaseMail, EMailFeasibilityDocument
from ..common.mail import BaseMail, EMailFeasibilityDocument
from ..cloud_storage import CloudStorage

gcs = CloudStorage()

# workers = Celery(__name__, broker=Config.BROKER_URL)
engine = db.create_engine(
    Config.SQLALCHEMY_DATABASE_URI, 
    isolation_level="AUTOCOMMIT",
    echo=True, 
    echo_pool="debug"
)

def send_complete_mail(session_id, time):
    try:
        query = """
        select 
            tu."name", 
            tu.email, 
            tus.project_name 
        from tbl_user_sessions tus 
        left join tbl_users tu on tu.id = tus.user_id 
        where tus.session_id = '{}'
        """.format(session_id)
        
        with sessionmaker(engine).begin() as session:
            data = session.execute(db.text(query))

            d = {}
            for row in data:
                d = row._asdict()
            
            mail_ = BaseMail(
                to=d.get('email'),
                subject=EMailFeasibilityDocument.SUBJECT,
                template=EMailFeasibilityDocument.TEMPLATE,
                data=d
            )
            mail_.send_brevo_mail()

            session.close()

    except Exception as e:
        print(str(e))

def register_document(session_id, time):

    document_id = None
    document_guid = str(uuid.uuid4())
    document_type = "General"
    document_status = "Final"
    document_name = session_id + '-' + time + '-templates.docx'
    client_name = "Feasibility Report - " + time

    query = """
    insert into "DocumentList" (
        project_id, document_id, document_type, 
        document_status, document_name, client_name,
        is_active
    ) values ('{}', '{}', '{}', '{}', '{}', '{}', 1) returning id;
    """.format(
        session_id, document_guid, document_type, 
        document_status, document_name, client_name
    )

    try:
        with sessionmaker(engine).begin() as session:
            data = session.execute(db.text(query))

            for row in data:
                document_id = row.id

            session.close()
    except(Exception, psycopg2.DatabaseError) as error:
        print(error)
    
    if document_id:
        send_complete_mail(session_id, time)
    
    return document_id

def check_base_folder_gcs_mount(_type: str = 'docx'): # sementara gapake ini
    base_path_csv = Config.GCS_MOUNT_PATH + '/generated-file/csv/'
    if not os.path.exists(base_path_csv):
        os.makedirs(base_path_csv)

    base_path_docx = Config.GCS_MOUNT_PATH + '/generated-file/{}/'.format(_type)
    if not os.path.exists(base_path_docx):
        os.makedirs(base_path_docx)
    
    return base_path_csv, base_path_docx

def check_base_folder(_type: str = 'docx'):
    base_path_csv = 'generated-file/csv/'
    if not os.path.exists(base_path_csv):
        os.makedirs(base_path_csv)

    base_path_docx = 'generated-file/{}/'.format(_type)
    if not os.path.exists(base_path_docx):
        os.makedirs(base_path_docx)
    
    return base_path_csv, base_path_docx

def generate_document(session_id, data):
    cur_time = datetime.now()
    date_time = cur_time.strftime("%Y%m%d%H%M%S")
    print(date_time)

    field_names = list(data.keys())

    base_path_csv, base_path_docx = check_base_folder()

    document_csv_path = base_path_csv + session_id + '-' + date_time + '-data.csv'
    with open (document_csv_path, 'w') as csvfile:
        writer = csv.DictWriter(csvfile, fieldnames=field_names)
        writer.writeheader() 
        writer.writerows([data])
    
    document_docx_path = base_path_docx + session_id + '-' + date_time + '-templates.docx'
    with open (document_docx_path, 'w') as csvfile:
        write_to_docx = fill_template_variables(
            session_id=session_id, 
            template_name="assets/general_purposes_template_v2.docx", 
            document_saved_path=document_docx_path, 
            context=data
        )
    
    gcs.upload(document_csv_path)
    gcs.upload(document_docx_path)

    document_id = register_document(session_id, date_time)

def generate_document_form(session_id, data):
    field_names = list(data.keys())

    base_path_csv, base_path_docx = check_base_folder(_type='docx-ccb')

    document_csv_path = base_path_csv + session_id + '-ccb-data.csv'
    with open (document_csv_path, 'w') as csvfile:
        writer = csv.DictWriter(csvfile, fieldnames=field_names)
        writer.writeheader() 
        writer.writerows([data])

    document_docx_path = base_path_docx + session_id + '-ccb-templates.docx'
    with open (document_docx_path, 'w') as csvfile:
        write_to_docx = fill_form_template_variables(
            session_id=session_id, 
            template_name="assets/ccb_template.docx", 
            document_saved_path=document_docx_path, 
            context=data
        )
    
    gcs.upload(document_csv_path)
    gcs.upload(document_docx_path)


# @workers.task
# def feasibility_template_task(session_id, data):
#     output = True

#     print('test')

#     generate_document(session_id, data)

#     return output

# @workers.task
# def form_template_task(session_id, data):
#     output = True

#     generate_document_form(session_id, data)

#     return output