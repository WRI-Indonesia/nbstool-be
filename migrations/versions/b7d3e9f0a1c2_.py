"""problem reports table

Revision ID: b7d3e9f0a1c2
Revises: a1b2c3d4e5f6
Create Date: 2026-09-25

"""
from alembic import op
import sqlalchemy as sa

# revision identifiers, used by Alembic.
revision = 'b7d3e9f0a1c2'
down_revision = 'a1b2c3d4e5f6'
branch_labels = None
depends_on = None


def upgrade():
    op.create_table(
        'tbl_logger_problem_reports',
        sa.Column('id', sa.Integer(), primary_key=True),
        sa.Column('session_id', sa.String(length=100), nullable=True),
        sa.Column('user_id', sa.Integer(), nullable=True),
        sa.Column('reporter', sa.String(length=255), nullable=True),
        sa.Column('description', sa.String(length=4000), nullable=True),
        sa.Column('page_url', sa.String(length=2048), nullable=True),
        sa.Column('locale', sa.String(length=16), nullable=True),
        sa.Column('user_agent', sa.String(length=1024), nullable=True),
        sa.Column('env', sa.String(length=128), nullable=True),
        sa.Column('screenshot_path', sa.String(length=255), nullable=True),
        sa.Column('created_at', sa.DateTime(), nullable=True),
    )
    op.create_index('ix_tbl_logger_problem_reports_session_id', 'tbl_logger_problem_reports', ['session_id'])
    op.create_index('ix_tbl_logger_problem_reports_user_id', 'tbl_logger_problem_reports', ['user_id'])


def downgrade():
    op.drop_index('ix_tbl_logger_problem_reports_user_id', table_name='tbl_logger_problem_reports')
    op.drop_index('ix_tbl_logger_problem_reports_session_id', table_name='tbl_logger_problem_reports')
    op.drop_table('tbl_logger_problem_reports')
