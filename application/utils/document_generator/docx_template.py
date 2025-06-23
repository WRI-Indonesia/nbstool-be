# scripts/template_generator.py
from docxtpl import DocxTemplate

import pathlib
import base64
import os

from ..cloud_storage import CloudStorage

gcs = CloudStorage()

def fill_template_variables(session_id, template_name, document_saved_path, context):
    doc = DocxTemplate(template_name)

    # put avatar / logo to image (document cover)
    if context['logo'] != '':
        filename = session_id + "_logo.jpg"
        user_folder = pathlib.Path("generated-file", "logo",  filename).resolve()

        if not os.path.exists(os.path.join('generated-file', 'logo')):
            os.makedirs(os.path.join('generated-file', 'logo'))

        logo_base64 = base64.b64decode(context["logo"], validate=True)
        with open(user_folder, "wb") as fh:
            fh.write(logo_base64)

        doc.replace_pic("logo-placeholder.jpg", user_folder)

        del context["logo"]
    
    # put project area map image to document (section 2)
    if context["tpl_section_2"]:
        filename = session_id + ".jpg"

        gcs.download(os.path.join("generated-file", "project-area", filename))
        user_folder = pathlib.Path("generated-file", "project-area",  filename).resolve()
        doc.replace_pic("map-placeholder.jpg", user_folder)

    # put deforestation graph image to document (section 2)
    if context["tpl_section_2"]:
        filename = session_id + "_deforestation_graph.jpg"

        gcs.download(os.path.join("generated-file", "graph", filename))
        deforestation_file_path = pathlib.Path("generated-file", "graph",  filename).resolve()
        doc.replace_pic("deforestation-placeholder.jpg", deforestation_file_path)

    # put precipitation graph image to document (section 3)
    if context["tpl_section_3"]:
        filename = session_id + "_precipitation_graph.jpg"

        gcs.download(os.path.join("generated-file", "graph", filename))
        precipitation_file_path = pathlib.Path("generated-file", "graph",  filename).resolve()
        doc.replace_pic("precipitation-placeholder.jpg", precipitation_file_path)

    # put temperature graph image to document (section 3)
    if context["tpl_section_3"]:
        filename = session_id + "_temperature_graph.jpg"

        gcs.download(os.path.join("generated-file", "graph", filename))
        temperature_file_path = pathlib.Path("generated-file", "graph",  filename).resolve()
        doc.replace_pic("temperature-placeholder.jpg", temperature_file_path)
    
    doc.render(context)

    doc.save(document_saved_path)

    return True

def fill_form_template_variables(session_id, template_name, document_saved_path, context):
    doc = DocxTemplate(template_name)

    # put project area map image to document (section 2)
    # if context.get('tpl_data') and context.get('tpl_data').get('tpl_section_2'):
    #     filename = session_id + ".jpg"
    #     user_folder = pathlib.Path("generated-file", "project-area",  filename).resolve()
    #     doc.replace_pic("map-placeholder.png", user_folder)
    
    doc.render(context)

    doc.save(document_saved_path)

    return True