"""project privacy level

Revision ID: a1b2c3d4e5f6
Revises: f7c2d81b4a90
Create Date: 2026-09-12

"""
from alembic import op
import sqlalchemy as sa

# revision identifiers, used by Alembic.
revision = 'a1b2c3d4e5f6'
down_revision = 'f7c2d81b4a90'
branch_labels = None
depends_on = None


def upgrade():
    op.add_column('tbl_user_sessions', sa.Column('privacy_level', sa.SmallInteger(), nullable=True, server_default='1'))


def downgrade():
    op.drop_column('tbl_user_sessions', 'privacy_level')
