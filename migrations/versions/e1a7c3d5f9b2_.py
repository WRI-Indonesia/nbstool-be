"""project deletion reminder stamp

Revision ID: e1a7c3d5f9b2
Revises: d9f5a1b2c3e4
Create Date: 2026-10-03

"""
from alembic import op
import sqlalchemy as sa

# revision identifiers, used by Alembic.
revision = 'e1a7c3d5f9b2'
down_revision = 'd9f5a1b2c3e4'
branch_labels = None
depends_on = None


def upgrade():
    op.add_column('tbl_user_sessions', sa.Column('deletion_reminder_sent_at', sa.DateTime(), nullable=True))


def downgrade():
    op.drop_column('tbl_user_sessions', 'deletion_reminder_sent_at')
