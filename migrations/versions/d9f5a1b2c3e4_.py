"""problem report analytics ids

Revision ID: d9f5a1b2c3e4
Revises: c8e4f0a1b2d3
Create Date: 2026-09-29

"""
from alembic import op
import sqlalchemy as sa

# revision identifiers, used by Alembic.
revision = 'd9f5a1b2c3e4'
down_revision = 'c8e4f0a1b2d3'
branch_labels = None
depends_on = None

COLUMNS = ('clarity_session_id', 'clarity_user_id', 'ga_client_id', 'ga_session_id', 'ga_user_id')


def upgrade():
    for name in COLUMNS:
        op.add_column('tbl_logger_problem_reports', sa.Column(name, sa.String(length=255), nullable=True))


def downgrade():
    for name in reversed(COLUMNS):
        op.drop_column('tbl_logger_problem_reports', name)
