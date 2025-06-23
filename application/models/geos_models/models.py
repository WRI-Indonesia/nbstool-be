# application/models/geos_models/models.py
from ... import db

import os

from geoalchemy2 import Geometry

from datetime import datetime, timedelta
from ...utils.common import get_date, map_attr

class Polygons(db.Model):
    __tablename__ = "polygons"
    id = db.Column(db.Integer, primary_key=True)
    geom = db.Column(Geometry(geometry_type='GEOMETRY', srid=4326))
    session_id = db.Column(db.String(100), nullable=False, index=True)

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


class MapExplorer(db.Model):
    __tablename__ = "MapExplorer"
    id = db.Column(db.Integer, primary_key=True)
    session_id = db.Column(db.String(100), nullable = False)
    project_duration = db.Column(db.Integer, nullable=False)
    estimated_unplanned_deforestation = db.Column(db.Float, nullable=False)
    rest_target = db.Column(db.PickleType, nullable=False)
    intervention = db.Column(db.PickleType, nullable=False)

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
 
    @classmethod
    def find_by_session_id(cls, session_id):
        return cls.query.filter_by(session_id=session_id).first()