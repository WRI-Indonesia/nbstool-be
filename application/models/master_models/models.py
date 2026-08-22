# application/models/master_models/models.py
from ... import db

import os

from datetime import datetime, timedelta
from ...utils.common import get_date, map_attr
from sqlalchemy.dialects.postgresql import JSONB

class Organization(db.Model):
    __tablename__ = 'tbl_organization_types'
    id = db.Column(db.Integer, primary_key=True)
    name = db.Column(db.String(200), nullable=False)
    is_active = db.Column(db.SmallInteger, default=False)
    created_at = db.Column(db.DateTime, default=get_date)
    created_by = db.Column(db.Integer, nullable=False)
    updated_at = db.Column(db.DateTime, nullable=True)

    @classmethod
    def find_by_id(cls, id):
        return cls.query.filter_by(id=id).first()

    @classmethod
    def find_by_is_active(cls, bool):
        return cls.query.filter_by(is_active=bool).all()

    def to_json(self, attr=[]):
        if attr:
            return map_attr(self, attr)
        
        return {
            'id': self.id,
            'name': self.name,
            'is_active': self.is_active,
        }


class DocumentList(db.Model):
    __tablename__ = "DocumentList"
    id = db.Column(db.Integer, primary_key=True)
    project_id = db.Column(db.String(100), nullable = False)
    document_id = db.Column(db.String(100), nullable = False)
    document_type = db.Column(db.String(100), nullable = False)
    document_status = db.Column(db.String(100), nullable = False)
    document_name = db.Column(db.String(255), nullable = False)
    client_name = db.Column(db.String(255), nullable = True)
    created_at = db.Column(db.DateTime, default=get_date)
    is_active = db.Column(db.Integer, default=1)

    @classmethod
    def find_by_project_id(cls, project_id):
        return cls.query.filter_by(project_id=project_id, is_active=1).order_by(DocumentList.created_at.desc()).all()

    @classmethod
    def find_by_project_id_and_document_type(cls, project_id, document_type, num_data = "1'"):
        if num_data == "all":
            return cls.query.filter_by(project_id=project_id, document_type=document_type, is_active=1).order_by(DocumentList.created_at.desc()).all()
        else:
            return cls.query.filter_by(project_id=project_id, document_type=document_type).order_by(DocumentList.created_at.desc()).first()
    
    @classmethod
    def find_by_document_id(cls, document_id):
        return cls.query.filter_by(document_id=document_id, is_active=1).first()


class DocumentData(db.Model):
    __tablename__ = "DocumentData"
    id = db.Column(db.Integer, primary_key=True)
    session_id = db.Column(db.String(100), nullable = False)
    certification_type = db.Column(db.String(100), nullable = False)
    section_1 = db.Column(db.PickleType)
    section_2 = db.Column(db.PickleType)
    section_3 = db.Column(db.PickleType)
    section_4 = db.Column(db.PickleType)
    section_5 = db.Column(db.PickleType)
    section_1_json = db.Column(JSONB, nullable=True)
    section_2_json = db.Column(JSONB, nullable=True)
    section_3_json = db.Column(JSONB, nullable=True)
    section_4_json = db.Column(JSONB, nullable=True)
    section_5_json = db.Column(JSONB, nullable=True)
    # v3 template form data (certification_type e.g. 'FeasibilityV3'): `form` holds the F03
    # socio-economic answers (se* keys), `user_input` the free overrides keyed by template tag
    # text. Seeded at /bind with analyser prefill, updated on save-draft / next, read by the docx
    # generation endpoint. The section_* columns above are the v2 CCB shape and stay untouched.
    form = db.Column(JSONB, nullable=True)
    user_input = db.Column(JSONB, nullable=True)
 

    @classmethod
    def find_by_session_id_and_type(cls, session_id, type):
        return cls.query.filter_by(session_id=session_id, certification_type=type).first()


class Dictionary(db.Model):
    __tablename__ = 'tbl_master_dictionary'
    id = db.Column(db.Integer, primary_key=True)
    code = db.Column(db.String(64), unique=True, nullable=False) # LOG_SUCCESS, LOG_FAILED | feature_input_polygon, feature_benefits
    name = db.Column(db.String(128), nullable=False) # Log Success            | Input Polygon, Benefits
    type = db.Column(db.String(32), nullable=False) # log type                | activity type, activity type
    description = db.Column(db.String(256), nullable=True) # keterangan       | Users Post/Request Polygon for input, Calculate Benefit
    
    rowstatus = db.Column(db.Integer, default=1)
    created_by = db.Column(db.String(100), nullable=True)
    created_date = db.Column(db.DateTime, default=get_date)
    modified_by = db.Column(db.String(100), nullable=True)
    modified_date = db.Column(db.DateTime, onupdate=get_date)

    def to_json(self, attr=[]):
        if attr:
            return map_attr(self, attr)
        
        return {
            'id': self.id,
            'code': self.code,
            'name': self.name,
            'type': self.type,
            'description': self.description,
            'rowstatus': self.rowstatus,
        }


class Settings(db.Model):
    __tablename__ = 'tbl_master_settings'
    id = db.Column(db.Integer, primary_key=True)
    name = db.Column(db.String(128), unique=True, nullable=False) # LOG_SUCCESS, LOG_FAILED | feature_input_polygon, feature_benefits
    value = db.Column(db.String(1024), nullable=False)


    @classmethod
    def find_by_name(self, name):
        return self.query.filter_by(name=name).first()