# application/models/master_models/models.py
from ... import db

import os

from datetime import datetime, timedelta
from ...utils.common import get_date, map_attr

class Logs(db.Model):
    __tablename__ = 'tbl_logger_logs'
    id = db.Column(db.Integer, primary_key=True)
    
    log_type_id = db.Column(db.Integer, index=True) # link ke master dictionary
    log_type_code = db.Column(db.String(64), nullable=True)
    log_type_name = db.Column(db.String(128), nullable=True)

    activity_type_id = db.Column(db.Integer, index=True) # link ke master dictionary
    activity_type_code = db.Column(db.String(64), nullable=True)
    activity_type_name = db.Column(db.String(128), nullable=True)

    description = db.Column(db.String(1000), nullable=True)
    
    session_id = db.Column(db.String(64), index=True)

    request_data = db.Column(db.PickleType) # requests
    response_data = db.Column(db.PickleType) # response
    
    created_at = db.Column(db.DateTime, default=get_date)
    created_by = db.Column(db.Integer, nullable=True)

    def to_json(self, attr=[]):
        if attr:
            return map_attr(self, attr)
        
        return {
            'id': self.id,
            
            'log_type_id': self.log_type_id,
            'log_type_code': self.log_type_code,
            'log_type_name': self.log_type_name,
            
            'activity_type_id': self.activity_type_id,
            'activity_type_code': self.activity_type_code,
            'activity_type_name': self.activity_type_name,

            'description': self.description,
            
            'session_id': self.session_id,

            'request_data': self.request_data,
            'response_data': self.response_data,

            'created_at': self.created_at.isoformat() if self.created_at else None,
            'created_by': self.created_by,
        }