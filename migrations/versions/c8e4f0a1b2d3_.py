"""user assistance_eligible flag

Revision ID: c8e4f0a1b2d3
Revises: b7d3e9f0a1c2
Create Date: 2026-09-25

"""
from alembic import op
import sqlalchemy as sa

# revision identifiers, used by Alembic.
revision = 'c8e4f0a1b2d3'
down_revision = 'b7d3e9f0a1c2'
branch_labels = None
depends_on = None


def upgrade():
    op.add_column('tbl_users', sa.Column('assistance_eligible', sa.Boolean(), nullable=False, server_default=sa.false()))


def downgrade():
    op.drop_column('tbl_users', 'assistance_eligible')
