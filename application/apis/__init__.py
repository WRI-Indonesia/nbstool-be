# application/apis/__init__.py
from flask import Blueprint, request, render_template
from flask_cors import cross_origin

apis_blueprint = Blueprint('apis', __name__, template_folder='_templates')

@apis_blueprint.route('/docs', methods=['GET'])
@cross_origin()
def landing():
    return render_template('index.html')

@apis_blueprint.route('/docs/specs', methods=['GET'])
@cross_origin()
def specs():
    return open('application/apis/_templates/specs.yaml', 'r').read().replace('http://someserversurl.com', '{}://{}'.format(request.scheme, request.host))
