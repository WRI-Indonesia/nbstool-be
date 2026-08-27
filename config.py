# config.py
import os
from dotenv import load_dotenv

dotenv_path = os.path.join(os.path.dirname(__file__), '.env')
if os.path.exists(dotenv_path):
    load_dotenv(dotenv_path)


class Config:
    SECRET_KEY = os.environ.get('SECRET_KEY')
    SQLALCHEMY_TRACK_MODIFICATIONS = False
    SQLALCHEMY_DATABASE_URI = os.environ.get('DB_SQLALCHEMY_URI')
    GIS_DB_CONSTRING = os.environ.get('GIS_DB_SQLALCHEMY_URI')
    
    # SQLALCHEMY_POOL_SIZE = 30 # custom pool size
    SQLALCHEMY_ENGINE_OPTIONS = {
        'pool_size': 30, # custom pool size
        'max_overflow': 0, # as recommended in https://docs.sqlalchemy.org/en/20/core/pooling.html
    }
    
    # SQLALCHEMY_BINDS = {
    #     'gis': os.environ.get('GIS_DB_SQLALCHEMY_URI')
    # }

    # FE_URL = os.environ.get('FE_URL')
    MAIL_BREVO_API_KEY = os.environ.get('MAIL_BREVO_API_KEY')
    # TOKEN_EXPIRE_HOURS = int(os.environ.get('TOKEN_EXPIRE_HOURS'))
    # TOKEN_EXPIRE_MINUTES = int(os.environ.get('TOKEN_EXPIRE_MINUTES'))

    # MAX_DRAW_AREA = int(os.environ.get('MAX_DRAW_AREA', 100000))

    # BENEFIT_API_URL = os.environ.get('BENEFIT_API_URL')

    # BROKER_URL = os.environ.get('BROKER_URL')

    GCS_MOUNT_PATH = os.environ.get('GCS_MOUNT_PATH')

    # moved to tbl_master_settings
    # RECAPTCHA_PROJECT_ID = os.environ.get('RECAPTCHA_PROJECT_ID')
    # RECAPTCHA_SITE_KEY = os.environ.get('RECAPTCHA_SITE_KEY')
    # RECAPTCHA_SCORE_THRESHOLD = float(os.environ.get('RECAPTCHA_SCORE_THRESHOLD', 0.8))

    GCS_BUCKET_NAME = os.environ.get('GCS_BUCKET_NAME')

    # INCUBATOR_EMAIL_LIST = os.environ.get('INCUBATOR_EMAIL_LIST')


class DevelopmentConfig(Config):
    ENV = "development"
    DEBUG = True
    SQLALCHEMY_ECHO = True


class UatConfig(Config):
    ENV = "uat"
    DEBUG = True
    SQLALCHEMY_ECHO = False


class ProductionConfig(Config):
    ENV = "production"
    DEBUG = False
    SQLALCHEMY_ECHO = False
