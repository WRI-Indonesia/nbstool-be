"""v3 document form storage + project description

Revision ID: f7c2d81b4a90
Revises: e3f1a9c07b21
Create Date: 2026-08-23

"""
from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects import postgresql

# revision identifiers, used by Alembic.
revision = 'f7c2d81b4a90'
down_revision = 'e3f1a9c07b21'
branch_labels = None
depends_on = None


def upgrade():
    op.add_column('DocumentData', sa.Column('form', postgresql.JSONB(astext_type=sa.Text()), nullable=True))
    op.add_column('DocumentData', sa.Column('user_input', postgresql.JSONB(astext_type=sa.Text()), nullable=True))
    op.add_column('tbl_user_sessions', sa.Column('project_description', sa.Text(), nullable=True))


def downgrade():
    op.drop_column('tbl_user_sessions', 'project_description')
    op.drop_column('DocumentData', 'user_input')
    op.drop_column('DocumentData', 'form')
