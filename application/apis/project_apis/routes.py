# application/apis/project_apis/routes.py
from flask import jsonify, request, make_response, g as g_var
from flask_login import current_user
from . import project_apis_blueprint
from ... import db
from ...models.user_models.models import UserSessions
from ...models.master_models.models import DocumentList, DocumentData, Organization
from ...models.geos_models.models import DataAnalyzer, Polygons, MapExplorer
from ...models.user_models.models import User

from datetime import datetime, timedelta
from flask_cors import cross_origin

import os
import gc
import json
import uuid
import string

from ...utils.common import AppMessageException, get_date, set_attr, get_default_list_param
from ...utils.common import app_exception_handler, success_handler
# off: v2 current-condition/benefit, the CCB template build read its analyzer shape
# from ...utils.geos.current_condition import process_get_data_analyzer
# from ...utils.document_generator import generate_document_form
#
# from ..geo_apis.utils import GeoLogic
from ...utils.document_generator.v3.prefill import feasibility_prefill


# legacy: /nbsapi/project-management/bind-project [POST]
@project_apis_blueprint.route('/bind', methods=['POST'])
@cross_origin()
def projects_bind_project():
    g_var.__api_name__ = 'projects_bind_project'

    g_var.__log_it__ = True
    g_var.__session_id__ = None
    g_var.__description_data__ = { 'project_name': '' }
    try:
        g_var.__request_data__ = request.get_json()
    except:
        pass

    try:
        if not current_user.is_authenticated:
            return make_response(jsonify(app_exception_handler('not logged in', 401)), 401)

        if not request.is_json:
            raise AppMessageException('please provide json data')
        
        data = request.get_json()

        session_id = data.get('session_id')
        user_id = current_user.id if current_user.is_authenticated else data.get('user_id')
        template_type = data.get('template_type')

        message = 'Project is updated successfully'
        status_code = 200

        g_var.__session_id__ = session_id

        document_id = str(uuid.uuid4())
        data_analyzer = DataAnalyzer.find_by_session_id(session_id)

        # v3 analyser shape: flat keys in site_information_json (the v2 pickle path is dead on
        # this branch and old rows will be migrated to v3).
        site = (data_analyzer.site_information_json if data_analyzer else None) or {}
        project_name = data.get('project_name')
        if not project_name:
            where = site.get('district') or site.get('province')
            country = site.get('country')
            if where and country:
                project_name = f"{string.capwords(where)} district Project in {string.capwords(country)}"
            else:
                project_name = f"Project {session_id[:8]}"

        g_var.__description_data__['project_name'] = project_name

        known_project = UserSessions.find_by_session_id(session_id)
        if not known_project:
            known_project = UserSessions()
            known_project.user_id = user_id
            known_project.session_id = session_id

            message = 'Project is created successfully'
            status_code = 201

        known_project.user_id = user_id
        known_project.project_name = project_name
        if data.get('description') is not None:
            known_project.project_description = data.get('description')
        known_project.is_project = 1
        db.session.add(known_project)

        # Seed the feasibility form storage with the analyser prefill, so the F03 form and the
        # docx template have their initial data from the moment the session becomes a project.
        # Never overwrites answers a user already saved.
        known_form = DocumentData.find_by_session_id_and_type(session_id, 'FeasibilityV3')
        if not known_form:
            known_form = DocumentData(session_id=session_id, certification_type='FeasibilityV3',
                                      form=feasibility_prefill(data_analyzer))
            db.session.add(known_form)

        if template_type is not None and str(template_type).lower() != 'general':
            # The CCB template build read the v2 current-condition analyzer shape
            # (process_get_data_analyzer), which is turned off. Disabled until the new
            # benefit/site-characterisation data feeds the templates.
            raise AppMessageException('fail, document templates are disabled')
        #     known_document_list = DocumentList.find_by_project_id_and_document_type(session_id, template_type)
        #
        #     if not known_document_list:
        #         known_document_list = DocumentList()
        #         known_document_list.project_id = session_id
        #         known_document_list.document_id = document_id
        #         known_document_list.document_type = template_type
        #         known_document_list.document_status = "Draft"
        #         known_document_list.document_name = '{}-ccb-templates.docx'.format(session_id)
        #         known_document_list.client_name = 'CCB Project Documentation'
        #
        #         known_document_data = DocumentData()
        #         known_document_data.session_id = session_id
        #         known_document_data.certification_type = template_type
        #
        #         db.session.add(known_document_list)
        #         db.session.add(known_document_data)
        #
        #         section_1 = dict()
        #         section_2 = dict()
        #         section_3 = dict()
        #         section_4 = dict()
        #         section_5 = dict()
        #
        #         # load data from processed current condition and benefit
        #         current_condition = process_get_data_analyzer(session_id)
        #         cc = json.loads(json.dumps(current_condition.get_json()))["result"]
        #
        #         # populate Basic Information section data
        #         section_1["doc_issued"] = datetime.now().strftime("%Y-%m-%d")
        #         section_1["project_country"] = cc["site_information"]["administrative_boundaries"]["country"]
        #         section_1["project_province"] = cc["site_information"]["administrative_boundaries"]["province"]
        #
        #         # populate General section data
        #         section_2["climate_description_textarea"] = "The Climate in project area experiences a tropical monsoonal climate, with two distinct rainy and dry seasons. For most of the year, there is significant rainfall in the study area (see Table of Climate). There is only a short dry season starting from June, which lasts through September. Average annual rainfall is {precipitation_mean} millimetres (mm). The lowest average rainfall is in August (peak of the dry season), while the highest average precipitation falls in January (peak of the wet season). The annual temperature averages around {temperature_mean}°C. The average highest temperature is in October ({temperature_min}°C), while the lowest average temperature occurs in January ({temperature_max}°C).".format(precipitation_mean = cc["climate"]["precipitation"]["mean"], temperature_min = cc["climate"]["temperature"]["min"], temperature_mean = cc["climate"]["temperature"]["mean"], temperature_max = cc["climate"]["temperature"]["max"])
        #
        #         ethnicity_list = ""
        #         ethnicity_list += os.linesep + '-' + " ".join(f" {ethnic}" for ethnic in cc["people"]["ethnicity"])
        #
        #         section_2["ethnicity_description_textarea"] = "Within this area, there are several groups of ethnicity, including:{ethnicities}".format(ethnicities=ethnicity_list)
        #         section_2["household"] = cc["people"]["demography"]["household"]
        #         section_2["indigenous_rights_textarea"] = "Activities being implemented under the [title of the project] project do not include any that will lead to the involuntary removal or relocation of property rights holders from their territories. All the activities are designed together with the community to agree on the best locations and according to the land uses designated in the forest permits."
        #         section_2["location_description_textarea"] = ""
        #         section_2["ongoing_dispute_textarea"] = "No ongoing disputes occurred during the implementation of the [title of project] project, either within or between communities or between communities and the concession holders around the project area."
        #         section_2["plant_description_textarea"] = "The {district} district, {province} province consists of (number) types of forests, such as (Ex. secondary dryland forest, secondary swamp forest, and secondary mangrove forest), as shown in Table of Forest Types and Map of Forest Types in project location.".format(district = cc["site_information"]["administrative_boundaries"]["district"], province = cc["site_information"]["administrative_boundaries"]["province"])
        #         section_2["proj_location_textarea"] = "The selected area is located in {district}, {province}, {country}".format(district = cc["site_information"]["administrative_boundaries"]["district"], province = cc["site_information"]["administrative_boundaries"]["province"], country = cc["site_information"]["administrative_boundaries"]["country"])
        #         section_2["project_zone_description_textarea"] = "{adm_bound}. {forest_cover}. {kba}. The selected area features {peat} ha of peatland and {mangrove} ha of mangrove, contributing to its ecological diversity.".format(adm_bound = cc["site_information"]["administrative_boundaries"]["adm_bound"], forest_cover = cc["site_information"]["administrative_boundaries"]["forest_cover"], kba = cc["nature"]["kba"]["text"], peat = cc["site_information"]["peatland_mangrove"]["peatland"], mangrove = cc["site_information"]["peatland_mangrove"]["mangrove"])
        #         section_2["without_project_textarea"] = "The forests in the project area are classified as [forest type 1], [forest type 2], and [forest type 3]. The most important characteristics of vegetation are described in Section 2.1.5.\n\nSee sections 3.1.4 and 3.1.5, which contain the description of the baseline scenario as well as the potential land use scenario and associated driers of land use changes most likely to occur within the project zone in the absence of the project.\n\nThe section lists alternative land use scenarios to the project activity resulting from the additionality analysis. The scenarios are: \n\n(please type - 500 words)"
        #
        #         # populate Climate section data
        #         section_3["proj_baseline_scenario"] = """
        #         The most plausible baseline scenario was determined using the VT0001 Tool for the Demonstration and Assessment of Additionality in VCS Agriculture, Forest and Other Land Use (AFOLU) Project Activities, Version 3.0. This tool is applicable to this project as the selection of the baseline scenario can be made from a stepwise approach consistent with determining the additionality of project activities. The tool was designed for AFOLU project activities and can be used under methodology VM0007.
        #         \n\n
        #         The methodological development for the selection of the project baseline was addressed as follows:\n
        #         1. Identification of alternative land use scenarios to the Project AFOLU activity: Alternative land use scenarios were identified with respect to the proposed project activities, and which could be defined as a baseline scenario. The rationale for the most likely landuse scenarios in the project area is presented below.\n
        #         2. identification of the barriers that impede the implementation of the proposed project activity which, in general terms, included financial barriers, institutional barriers, and technical barriers.\n
        #         3. show that the identified barriers do not prevent the implementation of at least one of the alternative land-use scenarios (except for the proposed project activity): within the alternative land-use scenarios, illegal mangrove logging for charcoal production and agriculture/farmland expansion by small-scale farmers are the most common land uses in the area and are found in most rural areas. However, agriculture/farmland expansion by small-scale farmers does not face significant barriers (see section 3.1.5)
        #         \n\n
        #         Barrier analysis is summarized in section 3.1.5. According to the information presented in the table, the possible baseline scenarios are illegal mangrove logging for household charcoal production and agriculture or farmland by small-scale farmers. However, only agriculture/farmland activity will be used to quantify emission reductions, as this scenario is associated with higher carbon content in the aboveground biomass and belowground biomass than illegal mangrove logging. Thus, it is ensured that ex-ante estimation is conservative according to the guideline of BL-UP module that potential baseline scenarios are avoided agriculture/farmland activity in forest areas
        #         """
        #         section_3["proj_definition_boundaries"] = "Considering that the areas from which information on the historical deforestation rate is extracted and projected into the future must be delimited by spatial and temporary limits, the delimitation of the RRD and the PA was carried out with consideration of the requirements of the VM0007 methodology, as described in section 3.3. The summary of the spatial boundaries and temporal boundaries are shown below:"
        #         section_3["proj_location_textarea"] = "Below is a description of the spatial boundaries of the project, considering three types of area used in the methodology. As the project is in the avoiding uplanned deforestation and degradation (AUDD) category, the unplanned deforestation area (PA), the reference region for projecting rate of deforestation (RRD), the reference region for projecting location of deforestation (RRL), and the leakage belt (LK) area are therefore considered."
        #
        #         # populate Community section data
        #         section_4["proj_deforestation_threat"] = "In this project area some Illegal activities, such as [illegal activites 1],  [illegal activites 2], [illegal activites 3], and [illegal activites 4] within forests have been historically practiced in parts of the project zone.\n\nBeside illegal activities, deforestation is also caused by several drivers such as: {dod}".format(dod = cc["site_information"]["driver_of_deforestation"]["driver_text"])
        #         section_4["proj_population_ethnicity"] = "Within this area, there are several groups of ethnicity, including:{ethnicities} \n\nProject developers should conduct a more detailed assessment about the list of ethnicity and proportion of each group population in the nearest districts/ municipalities, can add cultural, faith or religion, customary institution, and community relationship with natural resources information if any".format(ethnicities = ethnicity_list)
        #
        #         sectors = ""
        #         sectors += ", ".join(f" {ethnic}" for ethnic in cc["people"]["employment"]["top_3_sektor"])
        #
        #         section_4["proj_population_livelihood"] = "The unemployment rate in {district} District/City is {employment_rate}% with a total of {employment_num} unemployed. Majority of people in {district} District/City work in the {sector}.".format(district = cc["site_information"]["administrative_boundaries"]["district"], employment_rate = cc["people"]["employment"]["pengangguran_pct"], employment_num = cc["people"]["employment"]["pengangguran"], sector = sectors)
        #
        #         # populate Biodiversity section data
        #         section_5["proj_biodiversity_current_condition"] = "{kba}. According to the Forest Landscape Integrity Index, the average forest integrity in the regions is: {flii}. The project area has {integrity}, {meaning}\n\nThe selected area hosts a range of wildlife, including:\n- Birds: {bird}\n- Mammals: {mammal}\n- Amphibians: {amphibi}\n- Reptiles: {reptile}\n- Fish: {fish}\n\n{richness}.".format(kba = cc["nature"]["kba"]["text"], flii = cc["nature"]["flii"]["index"], integrity = cc["nature"]["flii"]["integrity"].lower(), meaning = cc["nature"]["flii"]["meaning"], bird = cc["nature"]["wildlife"]["bird"], mammal = cc["nature"]["wildlife"]["mammal"], amphibi = cc["nature"]["wildlife"]["amphibi"], reptile = cc["nature"]["wildlife"]["reptile"], fish = cc["nature"]["wildlife"]["marine_fish"], richness = cc["nature"]["richness"]["desc"])
        #         section_5["proj_optional_criteria"] = "The project does not seek to validate Gold Level for exceptional biodiversity benefits"
        #
        #         # save populated data into database
        #         document_data_exist = DocumentData.find_by_session_id_and_type(session_id, known_document_list.document_type)
        #
        #         if document_data_exist:
        #             document_data = DocumentData.query.filter_by(session_id=session_id, certification_type=known_document_list.document_type).first()
        #
        #             document_data.section_1 = section_1
        #             document_data.section_2 = section_2
        #             document_data.section_3 = section_3
        #             document_data.section_4 = section_4
        #             document_data.section_5 = section_5
        #
        #             db.session.add(document_data)
        #
        #             db.session.flush()
        #
        #             known_user_session = UserSessions.find_by_session_id(known_document_list.project_id)
        #             if not known_user_session:
        #                 known_user_session = UserSessions()
        #             known_user = User.query.filter_by(id=known_user_session.user_id).first()
        #             if not known_user:
        #                 known_user = User()
        #
        #             intervention = MapExplorer.find_by_session_id(known_document_list.project_id)
        #             if not intervention:
        #                 intervention = MapExplorer()
        #
        #             GeoLogic.handle_section_data(document_data)
        #
        #             data = {
        #                 'project_id': known_document_list.project_id,
        #                 'user': {
        #                     'fullname': known_user.name,
        #                     'email': known_user.email
        #                 },
        #                 'param': {
        #                     'project_duration': intervention.project_duration,
        #                     'estimated_unplanned_deforestation': intervention.estimated_unplanned_deforestation,
        #                     'rest_target': intervention.rest_target,
        #                 },
        #                 'section_1': document_data.section_1,
        #                 'section_2': document_data.section_2,
        #                 'section_3': document_data.section_3,
        #                 'section_4': document_data.section_4,
        #                 'section_5': document_data.section_5,
        #
        #                 'tpl_data': GeoLogic.get_template_data(session_id, [n for n in range(1, 6) if eval('document_data.section_{}'.format(n))]),
        #             }
        #
        #             # tasks = form_template_task.delay(session_id, data)
        #             generate_document_form(session_id, data)

        db.session.commit()

        results = {
            'message': message,
            'project_id': session_id,
            'document_id': document_id,
            'project_name': project_name
        }


        return make_response(jsonify(success_handler({ 'result': results })), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# ---------------------------------------------------------------------------------------------
# v3 project readers. The dashboard (F04) and detail (F04.1)
# screens read the v3 JSONB shapes; the old readers went through the v2 pickle columns and
# crash on a v3 row. UI data with no backend source yet -- collaborators, monitoring entries,
# privacy level, history/audit log -- is simply absent from the payload until its feature
# exists; the frontend skips what is not there.
# ---------------------------------------------------------------------------------------------

_ECO_SELECTION_KEYS = {'forest': 'Forest', 'mangrove': 'Mangrove', 'peatland': 'Peatland'}
_PATHWAY_ORDER = ('Protect', 'Manage', 'Restore')


def _pathway_json(analyzer):
    """The v3 pathway result: one object. Legacy rows store a LIST in this column (the v2
    shape) and carry none of the v3 fields, so anything non-dict reads as empty."""
    pathway = (analyzer.intervention_eligibility_json if analyzer else None) or {}
    return pathway if isinstance(pathway, dict) else {}


def _project_ecosystems(analyzer):
    """`[{name, key, pathways}]` for the dashboard cards and the Ecosystem filter.

    The user's pathway-screen selections (persisted with the benefit run) are what the project
    chose, so they win; before a benefit run, the eligibility cards stand in (any intervention
    with area on the site)."""
    benefit = (getattr(analyzer, 'benefit_json', None) if analyzer else None) or {}
    selections = (benefit.get('assumptions') or {}).get('selections') or {}
    items = []
    for key, name in _ECO_SELECTION_KEYS.items():
        chosen = selections.get(key) or {}
        pathways = [pw for pw in _PATHWAY_ORDER if chosen.get(pw.lower())]
        if pathways:
            items.append({'name': name, 'key': key, 'pathways': pathways})
    if items:
        return items
    pathway = _pathway_json(analyzer)
    for card in pathway.get('ecosystems') or []:
        eligible = {i.get('intervention') for i in card.get('interventions') or []
                    if (i.get('area_ha') or 0) > 0}
        pathways = [pw for pw in _PATHWAY_ORDER if pw in eligible]
        if pathways:
            label = card.get('label') or ''
            items.append({'name': label, 'key': label.lower(), 'pathways': pathways})
    return items


def _project_area_ha(analyzer, session_id):
    """Total site area: the pathway run's figure, else the polygon's own size."""
    area = _pathway_json(analyzer).get('project_area_ha')
    if area is not None:
        return area
    polygon = Polygons.query.filter_by(session_id=session_id).first()
    return polygon.project_area_size if polygon else None


def _monitoring_status(session_id):
    """The dashboard's Monitoring Status filter values: a saved F05 plan makes the project
    'Monitoring Active', anything earlier is 'Waiting for Monitoring Plan'. MRV entry counts
    would refine this later; F06 has no backend yet."""
    doc = DocumentData.find_by_session_id_and_type(session_id, 'MonitoringV3')
    form = doc.form if doc and isinstance(doc.form, dict) else {}
    if form.get('mpPlan'):
        return 'Monitoring Active'
    return 'Waiting for Monitoring Plan'


def _iso(dt):
    return dt.isoformat() if dt else None


# legacy: /nbsapi/project-management/project-list [POST]
@project_apis_blueprint.route('/list', methods=['GET'])
@cross_origin()
def projects_list():
    g_var.__api_name__ = 'projects_list'

    try:
        if not current_user.is_authenticated:
            return make_response(jsonify(app_exception_handler('not logged in', 401)), 401)

        known_organization = Organization.find_by_id(id=current_user.organization_type_id)
        if not known_organization:
            known_organization = Organization()

        projects = UserSessions.query.filter_by(user_id=current_user.id, is_active=1, is_project=True, analyzer_version='v3').order_by(db.desc(UserSessions.created_at)).all()
        ids = [p.session_id for p in projects]

        # FOUR round-trips total, whatever the project count -- analyzers, document counts,
        # monitoring-plan existence and polygon areas each come back in one batched query and
        # join in Python by session_id. (The first cut queried all three per project in the
        # loop; the tables have no FK relationships to join through, so batched IN() lookups
        # are the join here.)
        analyzers: dict = {}
        doc_counts: dict = {}
        monitored: set = set()
        polygon_areas: dict = {}
        if ids:
            for analyzer in DataAnalyzer.query.filter(DataAnalyzer.session_id.in_(ids)):
                analyzers.setdefault(analyzer.session_id, analyzer)
            doc_counts = dict(
                db.session.query(DocumentList.project_id, db.func.count(DocumentList.id))
                .filter(DocumentList.project_id.in_(ids), DocumentList.is_active == 1)
                .group_by(DocumentList.project_id).all())
            monitored = {sid for (sid,) in db.session.query(DocumentData.session_id)
                         .filter(DocumentData.session_id.in_(ids),
                                 DocumentData.certification_type == 'MonitoringV3',
                                 DocumentData.form['mpPlan'].astext.isnot(None))}
            for sid, area in (db.session.query(Polygons.session_id, Polygons.project_area_size)
                              .filter(Polygons.session_id.in_(ids))):
                polygon_areas.setdefault(sid, area)

        # One row per project with every field the dashboard's toolbar works on -- status /
        # ecosystem / country filters, name+location search, last-updated and area sorts --
        # so the client filters and sorts locally exactly like the mock does.
        items = []
        for project in projects:
            analyzer = analyzers.get(project.session_id)
            site = (analyzer.site_information_json if analyzer else None) or {}
            area = _pathway_json(analyzer).get('project_area_ha')
            if area is None:
                area = polygon_areas.get(project.session_id)

            items.append({
                'project_id': project.session_id,
                'project_name': project.project_name,
                'project_description': project.project_description,
                'country': site.get('country'),
                'province': site.get('province'),
                'area_ha': area,
                'ecosystems': _project_ecosystems(analyzer),
                'monitoring_status': ('Monitoring Active' if project.session_id in monitored
                                      else 'Waiting for Monitoring Plan'),
                'documents': doc_counts.get(project.session_id, 0),
                'created_at': _iso(project.created_at),
                'updated_at': _iso(project.updated_at or project.created_at),
            })

        results = {
            'user': {
                'user_fullname': current_user.name,
                'user_email': current_user.email,
                'user_org_type': known_organization.name,
                'user_org_name': current_user.organization_name,
            },
            # The profile-summary stats. Monitoring entries belong here too once F06 exists.
            'stats': {
                'total_projects': len(items),
                'area_covered_ha': sum(i['area_ha'] or 0 for i in items),
            },
            'projects': items
        }

        return make_response(jsonify(success_handler({ 'result': results }, status_code=200)), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# legacy: /nbsapi/project-management/project-detail [POST]
@project_apis_blueprint.route('', methods=['GET'])
@cross_origin()
def projects_details():
    g_var.__api_name__ = 'projects_details'

    try:
        if not current_user.is_authenticated:
            return make_response(jsonify(app_exception_handler('not logged in', 401)), 401)

        data = request.args

        session_id = data.get('project_id')

        known_polygons = Polygons.query.filter_by(session_id=session_id).first()
        if not known_polygons:
            raise AppMessageException('fail, session id Not found')
        
        geom = Polygons.get_geometry(session_id).first()
        geom = json.loads(geom[0])

        known_project = UserSessions.find_by_session_id(session_id)
        if not known_project:
            raise AppMessageException('fail, project Not found')
        analyzer = DataAnalyzer.find_by_session_id(session_id)
        site = (analyzer.site_information_json if analyzer else None) or {}
        benefit = (getattr(analyzer, 'benefit_json', None) if analyzer else None) or {}
        assumptions = benefit.get('assumptions') or {}
        selections = assumptions.get('selections') or {}

        # NbS Intervention block: chosen activity count from the pathway-screen selections;
        # duration from the benefit run's assumptions. Both null until benefit has run.
        activities_selected = sum(
            len((selections.get(key) or {}).get('activities') or [])
            for key in _ECO_SELECTION_KEYS) or None

        # Monitoring-plan card: counts out of the saved F05 plan. Entries logged and
        # monitoring frequency summaries wait for F06.
        monitoring_plan = None
        mp_doc = DocumentData.find_by_session_id_and_type(session_id, 'MonitoringV3')
        mp_form = mp_doc.form if mp_doc and isinstance(mp_doc.form, dict) else {}
        mp_plan = mp_form.get('mpPlan')
        if mp_plan:
            activities = [a for eco in mp_plan.get('ecos') or []
                          for a in eco.get('activities') or []]
            monitoring_plan = {
                'activities': len(activities),
                'indicators': sum(len(g.get('items') or [])
                                  for a in activities for g in a.get('groups') or []),
            }

        documents = [
            {
                'document_id': d.document_id,
                'document_type': d.document_type,
                'document_status': d.document_status,
                'document_name': d.client_name,
                'created_at': _iso(d.created_at),
            }
            for d in DocumentList.find_by_project_id(session_id) or []
        ]

        # No backend data yet, so no field: collaborators, privacy level, history/audit log,
        # MRV entries.
        results = {
            'polygon': geom,
            'project_id': session_id,
            'project_name': known_project.project_name,
            'project_description': known_project.project_description,
            'country': site.get('country'),
            'province': site.get('province'),
            'district': site.get('district'),
            'area_ha': _project_area_ha(analyzer, session_id),
            'monitoring_status': _monitoring_status(session_id),
            'ecosystems': _project_ecosystems(analyzer),
            'intervention': {
                'duration_years': assumptions.get('duration_years'),
                'activities_selected': activities_selected,
            },
            'documents': documents,
            'monitoring_plan': monitoring_plan,
            'created_at': _iso(known_project.created_at),
            'updated_at': _iso(known_project.updated_at or known_project.created_at),
            # The Analysis tab's data, from the analyzer row this response already loaded --
            # each section exactly as its stream emitted it (sitechar columns flat, threat
            # nested per tab, pathway one document, benefit nested per process; null = never
            # ran). No separate endpoint: the old /geos/feature/data-analyzer getter was
            # removed in favour of this.
            'analysis': {
                'site_information': analyzer.site_information_json if analyzer else None,
                'nature': analyzer.nature_json if analyzer else None,
                'climate': analyzer.climate_json if analyzer else None,
                'people': analyzer.people_json if analyzer else None,
                'threat': analyzer.threat_json if analyzer else None,
                'pathway': analyzer.intervention_eligibility_json if analyzer else None,
                'benefit': analyzer.benefit_json if analyzer else None,
            },
        }

        return make_response(jsonify(success_handler({ 'result': results }, status_code=200)), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# legacy: /nbsapi/project-management/project-edit [POST]
@project_apis_blueprint.route('', methods=['POST', 'PUT'])
@cross_origin()
def projects_update():
    g_var.__api_name__ = 'projects_update'

    try:
        if not current_user.is_authenticated:
            return make_response(jsonify(app_exception_handler('not logged in', 401)), 401)

        if not request.is_json:
            raise AppMessageException('please provide json data')
        
        data = request.get_json()

        project_id = data.get('project_id')
        project_name = data.get('project_name')
        
        known_project = UserSessions.find_by_session_id(project_id)
        if not known_project:
            raise AppMessageException('fail. project not found')
        
        known_project.project_name = project_name

        db.session.add(known_project)
        db.session.commit()
        
        results = {
            'message': 'Project is updated successfully',
            'project_id': project_id,
            'project_name': project_name
        }

        return make_response(jsonify(success_handler({ 'result': results }, status_code=200)), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# legacy: /nbsapi/project-management/project-delete [POST]
@project_apis_blueprint.route('', methods=['DELETE'])
@cross_origin()
def projects_delete():
    g_var.__api_name__ = 'projects_delete'

    g_var.__log_it__ = True
    g_var.__session_id__ = None
    g_var.__description_data__ = { 'project_name': '' }
    try:
        g_var.__request_data__ = request.get_json()
    except:
        pass
    try:
        if not current_user.is_authenticated:
            return make_response(jsonify(app_exception_handler('not logged in', 401)), 401)

        if not request.is_json:
            raise AppMessageException('please provide json data')
        
        data = request.get_json()

        project_id = data.get('project_id')

        g_var.__session_id__ = project_id

        known_project = UserSessions.find_by_session_id(project_id)
        if not known_project:
            raise AppMessageException('fail. project not found')
        
        g_var.__description_data__['project_name'] = known_project.project_name
        
        known_project.is_active = 0
        known_project.updated_by = current_user.id

        db.session.add(known_project)
        db.session.commit()
        
        return make_response(jsonify(success_handler({ }, message='project is deleted successfully', status_code=200)), 200)
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error


# legacy: /nbsapi/project-management/project-documents [POST]
@project_apis_blueprint.route('/documents', methods=['GET'])
@cross_origin()
def projects_documents():
    g_var.__api_name__ = 'projects_documents'

    try:
        if not current_user.is_authenticated:
            return make_response(jsonify(app_exception_handler('not logged in', 401)), 401)

        data = request.args

        project_id = data.get('project_id')
        
        known_document_list = DocumentList.find_by_project_id(project_id)
        
        items = []
        if known_document_list:
            for d in known_document_list:
                items.append({
                    'document_id': d.document_id,
                    'document_type': d.document_type,
                    'document_status': d.document_status,
                    'document_name': d.client_name,
                    'project_id': d.project_id,
                })
        
        return items
    except AppMessageException as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 400) # send bad request
    except Exception as e:
        return make_response(jsonify(app_exception_handler(e, services=g_var.__api_name__)), 500) # send internal error