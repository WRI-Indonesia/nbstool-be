"""v3 analyzer persistence: threat column + engine version flag

Revision ID: e3f1a9c07b21
Revises: dcb16f2f772f
Create Date: 2026-08-22

"""
from alembic import op
import sqlalchemy as sa
from sqlalchemy.dialects import postgresql

# revision identifiers, used by Alembic.
revision = 'e3f1a9c07b21'
down_revision = 'dcb16f2f772f'
branch_labels = None
depends_on = None


def upgrade():
    op.add_column('DataAnalyzer', sa.Column('threat_json', postgresql.JSONB(astext_type=sa.Text()), nullable=True))
    op.add_column('tbl_user_sessions', sa.Column('analyzer_version', sa.String(length=10), nullable=True))


def downgrade():
    op.drop_column('tbl_user_sessions', 'analyzer_version')
    op.drop_column('DataAnalyzer', 'threat_json')
