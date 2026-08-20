# application/__init__.py
import config
import os, gc
from flask import Flask
from flask_login import LoginManager
from flask_sqlalchemy import SQLAlchemy
from flask_jwt_extended import JWTManager

from flask_cors import CORS

db = SQLAlchemy()
login_manager = LoginManager()
jwt = JWTManager()

def create_app():
    app = Flask(__name__)
    cors = CORS(app)
    environment_configuration = os.environ['CONFIGURATION_SETUP']
    app.config.from_object(environment_configuration)
    app.config['CORS_HEADERS'] = 'Content-Type'
    app.config['RESTX_MASK_SWAGGER'] = False

    db.init_app(app)
    login_manager.init_app(app)
    jwt.init_app(app)

    with app.app_context():
        @app.route('/', methods=['GET']) # for healthcheck
        def check():
            return 'ok'

        # Register blueprints
        from .apis.user_apis import user_apis_blueprint
        app.register_blueprint(user_apis_blueprint, url_prefix='/users')

        from .apis.master_apis import master_apis_blueprint
        app.register_blueprint(master_apis_blueprint, url_prefix='/masters')

        from .apis.mail_apis import mail_apis_blueprint
        app.register_blueprint(mail_apis_blueprint, url_prefix='/mails')

        from .apis.geo_apis import geo_apis_blueprint
        app.register_blueprint(geo_apis_blueprint, url_prefix='/geos')

        from .apis.project_apis import project_apis_blueprint
        app.register_blueprint(project_apis_blueprint, url_prefix='/projects')

        from .apis.logger_apis import logger_apis_blueprint
        app.register_blueprint(logger_apis_blueprint, url_prefix='/loggers')

        from .apis.document_apis import document_apis_blueprint
        app.register_blueprint(document_apis_blueprint, url_prefix='/documents')

        from .apis.incubator_apis import incubator_apis_blueprint
        app.register_blueprint(incubator_apis_blueprint, url_prefix='/incubators')

        from .apis import apis_blueprint
        app.register_blueprint(apis_blueprint, url_prefix='/api/v1')

        # from .apis.gis_apis import gis_apis_blueprint
        # app.register_blueprint(gis_apis_blueprint, url_prefix='/giss')

        # so tricky, if this placed before the first 'users' api,
        # the prefix /users will be overriden by this one
        # so it will return null if we call /users/something

        # from .apis import swagger_apis_blueprint
        # app.register_blueprint(swagger_apis_blueprint, url_prefix='/')
    
    return app
