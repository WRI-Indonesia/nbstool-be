# application/models/user_models/models.py
from ... import db
from ..master_models.models import Settings

from flask import current_app
from flask_login import UserMixin
from passlib.hash import sha256_crypt
import uuid

import os
import jwt

from datetime import datetime, timedelta
from ...utils.common import get_date, map_attr

# old
from flask_bcrypt import Bcrypt
from sqlalchemy.ext.hybrid import hybrid_property
from sqlalchemy.dialects.postgresql import JSONB
# end old import

bcrypt = Bcrypt()

class User(UserMixin, db.Model):
    __tablename__ = 'tbl_users'
    id = db.Column(db.Integer, primary_key=True)
    email = db.Column(db.String(255), unique=True, nullable=False)
    name = db.Column(db.String(200), nullable=False)
    password = db.Column(db.String(100), nullable=False)
    created_at = db.Column(db.DateTime, default=get_date)
    # admin = db.Column(db.Boolean, default=False)
    organization_type_id = db.Column(db.Integer)
    organization_name = db.Column(db.String(255), nullable=False)
    is_active = db.Column(db.Boolean, default=False)
    permission_policy = db.Column(db.Integer)
    extended_data = db.Column(db.PickleType, nullable=True)
    extended_data_json = db.Column(JSONB, nullable=True)
    avatar = db.Column(db.LargeBinary, nullable=True)
    size_limit = db.Column(db.Float, default=100000)

    # public_id = db.Column(db.String(36), unique=True, default=lambda: str(uuid4()))

    def encode_password(self):
        log_rounds = current_app.config.get("BCRYPT_LOG_ROUNDS")
        hash_bytes = bcrypt.generate_password_hash(self.password, log_rounds)
        self.password = hash_bytes.decode("utf-8")
    
    def check_password(self, password):
        return bcrypt.check_password_hash(self.password, password)
    
    def encode_access_token(self):
        now = get_date()
        TOKEN_EXPIRE_HOURS = Settings.find_by_name(name='TOKEN_EXPIRE_HOURS')
        TOKEN_EXPIRE_MINUTES = Settings.find_by_name(name='TOKEN_EXPIRE_MINUTES')
        try:
            token_age_h = int(TOKEN_EXPIRE_HOURS.value)
            token_age_m = int(TOKEN_EXPIRE_MINUTES.value)
        except Exception as e:
            current_app.logger.info('user models: {}'.format(str(e)))
            raise Exception('encode access token settings invalid or not found')
        expire = now + timedelta(hours=token_age_h, minutes=token_age_m)
        expires_in_seconds = token_age_h * 3600 + token_age_m * 60
        payload = dict(exp=expire, iat=now, user_id=self.id)
        key = current_app.config.get("SECRET_KEY")
        return jwt.encode(payload, key, algorithm="HS256"), expires_in_seconds
    
    def encode_refresh_token(self):
        now = get_date()
        expire = now + timedelta(hours=24, minutes=0)
        payload = dict(exp=expire, iat=now, user_id=self.id)
        key = current_app.config.get("SECRET_KEY")
        return jwt.encode(payload, key, algorithm="HS256")

    # @hybrid_property
    # def created_at_str(self):
    #     created_at_utc = make_tzaware(
    #         self.created_at, use_tz=timezone.utc, localize=False
    #     )
    #     return localized_dt_string(created_at_utc, use_tz=get_local_utcoffset())

    @staticmethod
    def decode_access_token(access_token):
        if isinstance(access_token, bytes):
            access_token = access_token.decode("ascii")
        if access_token.startswith("Bearer "):
            split = access_token.split("Bearer")
            access_token = split[1].strip()
        try:
            key = current_app.config.get("SECRET_KEY")
            payload = jwt.decode(access_token, key, algorithms=["HS256"])
        except jwt.ExpiredSignatureError:
            error = "Access token expired. Please log in again."
            return Result.Fail(error)
        except jwt.InvalidTokenError:
            error = "Invalid token. Please log in again."
            return Result.Fail(error)

        user_dict = dict(
            user_id=payload["user_id"],   
            # admin=payload["admin"],
            token=access_token,
            expires_at=payload["exp"],
        )
        return Result.Ok(user_dict)

    @staticmethod
    def decode_refresh_token(refresh_token):
        if isinstance(refresh_token, bytes):
            refresh_token = refresh_token.decode("ascii")
        if refresh_token.startswith("Bearer "):
            split = refresh_token.split("Bearer")
            refresh_token = split[1].strip()

        try:
            key = current_app.config.get("SECRET_KEY")
            payload = jwt.decode(refresh_token, key, algorithms=["HS256"])
        except jwt.ExpiredSignatureError:
            error = "Access token expired. Please log in again."
            return Result.Fail(error)
        except jwt.InvalidTokenError:
            error = "Invalid token. Please log in again."
            return Result.Fail(error)

        user_dict = dict(
            user_id=payload["user_id"],   
            # admin=payload["admin"],
            token=refresh_token,
            expires_at=payload["exp"],
        )

        return Result.Ok(user_dict)

    @classmethod
    def find_by_id(cls, id):
        return cls.query.filter_by(id=id).first()
    

    def to_json(self, attr=[]):
        if attr:
            return map_attr(self, attr)
        
        return {
            'name': self.name,
            'email': self.email,
            'organization_type_id': self.organization_type_id,
            'organization_name': self.organization_name,
            'permission_policy': self.permission_policy,
            'avatar': self.avatar,
        }
    
    # @classmethod
    # def find_by_organization_name(cls, organization_name):
    #     return cls.query.filter_by(organization_name=)


class SessionsAuth(db.Model):
    __tablename__ = "tbl_sessions"
    id = db.Column(db.Integer, primary_key=True)
    session_id = db.Column(db.String(100), nullable=False, index=True)
    created_at = db.Column(db.DateTime, default=get_date)
    aoi_total_area = db.Column(db.Float)
    aoi = db.Column(db.String(100))
    baseline_id = db.Column(db.Integer)
    intervention_id = db.Column(db.Integer)
    intervention_parameter_value = db.Column(db.String(100))
    benefit_id = db.Column(db.Integer)
    is_active = db.Column(db.Integer, default=1)

    @classmethod
    def find_by_session_id(cls, session_id):
        return cls.query.filter_by(session_id=session_id).first()
    
    @hybrid_property
    def created_at_str(self):
        created_at_utc = make_tzaware(
            self.created_at, use_tz=timezone.utc, localize=False
        )
        return localized_dt_string(created_at_utc, use_tz=get_local_utcoffset())


class UserSessions(db.Model):
    __tablename__ = "tbl_user_sessions"
    id = db.Column(db.Integer, primary_key=True)
    user_id = db.Column(db.Integer)
    session_id = db.Column(db.String(100), nullable = False)
    is_active = db.Column(db.Integer, default=1)
    created_at = db.Column(db.DateTime, default=get_date)
    project_name = db.Column(db.String(100), nullable = True)

    is_project = db.Column(db.Boolean, default=False)
    analyzer_version = db.Column(db.String(10), nullable=True)  # NULL = legacy/v2, 'v3' = v3 engine

    updated_at = db.Column(db.DateTime, onupdate=get_date)
    updated_by = db.Column(db.Integer, default=0)

    @classmethod
    def find_by_session_id(cls, session_id):
        return cls.query.filter_by(session_id=session_id, is_active=1).first()

    @classmethod
    def find_by_user_id(cls, user_id):
        return cls.query.filter_by(user_id=user_id, is_active=1).order_by(UserSessions.created_at.desc()).all()
    
    @classmethod
    def find_by_session_id_is_project(cls, session_id):
        return cls.query.filter_by(session_id=session_id, is_active=1, is_project=True).first()