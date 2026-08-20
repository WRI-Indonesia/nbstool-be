# application/models/geos_models/models.py
from ... import db
from ..master_models.models import Settings

from flask_login import current_user

import os

from geoalchemy2 import Geometry
from sqlalchemy.dialects.postgresql import JSONB

from datetime import datetime, timedelta
from ...utils.common import AppMessageException
from ...utils.common import get_date, map_attr

class Polygons(db.Model):
    __tablename__ = "polygons"
    id = db.Column(db.Integer, primary_key=True)
    geom = db.Column(Geometry(geometry_type='GEOMETRY', srid=4326))
    session_id = db.Column(db.String(100), nullable=False, index=True)
    project_area_size = db.Column(db.Numeric(12, 2, asdecimal=False, decimal_return_scale=None), nullable=True)
    country = db.Column(db.String(128), nullable=True)
    province = db.Column(db.String(128), nullable=True)
    district = db.Column(db.String(128), nullable=True)

    @classmethod
    def find_by_session_id(cls, session_id):
        return cls.query.filter_by(session_id=session_id).first() 
    
    @classmethod
    def get_geometry(cls, session_id):
        query = db.session.query(db.func.ST_AsGeoJSON(cls.geom)).filter(cls.session_id == session_id)
        return query
    
    @classmethod
    def replace_geometry(cls, session_id, geom, gdf):
        geometry_ = cls.query.filter_by(session_id=session_id).first()
        geometry_.geom = geom
        geometry_.geom = gdf
        return geometry_
    
    def assert_area_size(self):
        size_limit = int(Settings.find_by_name(name='MAX_DRAW_AREA').value)
        if current_user.is_authenticated:
            size_limit = current_user.size_limit if current_user.size_limit else size_limit
        
        if self.project_area_size and int(self.project_area_size) > size_limit:
            raise AppMessageException('project area exceed user size area limit')


class MapExplorer(db.Model):
    __tablename__ = "MapExplorer"
    id = db.Column(db.Integer, primary_key=True)
    session_id = db.Column(db.String(100), nullable = False)
    project_duration = db.Column(db.Integer, nullable=False)
    estimated_unplanned_deforestation = db.Column(db.Float, nullable=False)
    rest_target = db.Column(db.PickleType, nullable=False)
    intervention = db.Column(db.PickleType, nullable=False)
    rest_target_json = db.Column(JSONB, nullable=True)
    intervention_json = db.Column(JSONB, nullable=True)

    @classmethod
    def find_by_session_id(cls, session_id):
        return cls.query.filter_by(session_id=session_id).first()
    
    def to_json(self, attr=[]):
        if attr:
            return map_attr(self, attr)
        
        return {
            'session_id': self.session_id,
            'project_duration': self.project_duration,
            'estimated_unplanned_deforestation': self.estimated_unplanned_deforestation,
            'rest_target': self.rest_target,
            'intervention': self.intervention,
        }


class DataAnalyzer(db.Model):
    __tablename__ = "DataAnalyzer"
    id = db.Column(db.Integer, primary_key=True)
    session_id = db.Column(db.String(100), nullable = False)
    site_information = db.Column(db.PickleType, nullable=True)
    nature = db.Column(db.PickleType, nullable=True)
    climate = db.Column(db.PickleType, nullable=True)
    people = db.Column(db.PickleType, nullable=True)
    benefit = db.Column(db.PickleType, nullable=True)
    intervention_eligibility = db.Column(db.PickleType, nullable=True)
    site_information_json = db.Column(JSONB, nullable=True)
    nature_json = db.Column(JSONB, nullable=True)
    climate_json = db.Column(JSONB, nullable=True)
    people_json = db.Column(JSONB, nullable=True)
    benefit_json = db.Column(JSONB, nullable=True)
    intervention_eligibility_json = db.Column(JSONB, nullable=True)
 
    @classmethod
    def find_by_session_id(cls, session_id):
        return cls.query.filter_by(session_id=session_id).first()