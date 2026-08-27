from google.cloud import recaptchaenterprise_v1

from ..common import AppMessageException
from ...models.master_models.models import Settings

class CloudRecaptcha():

    def __init__(self):
        self.recaptcha_client = recaptchaenterprise_v1.RecaptchaEnterpriseServiceClient()

    def verify(self, token, action):
        settings = {
            setting.name: setting.value
            for setting in Settings.query.filter(Settings.name.in_(
                ['RECAPTCHA_PROJECT_ID', 'RECAPTCHA_SITE_KEY', 'RECAPTCHA_SCORE_THRESHOLD']
            )).all()
        }
        project_id = settings.get('RECAPTCHA_PROJECT_ID')
        site_key = settings.get('RECAPTCHA_SITE_KEY')

        if not project_id or not site_key:
            return # reCAPTCHA not configured for this environment, skip verification

        if not token:
            raise AppMessageException('please input: recaptcha_token (text mandatory)')

        event = recaptchaenterprise_v1.Event()
        event.site_key = site_key
        event.token = token

        assessment = recaptchaenterprise_v1.Assessment()
        assessment.event = event

        request = recaptchaenterprise_v1.CreateAssessmentRequest()
        request.assessment = assessment
        request.parent = f"projects/{project_id}"

        response = self.recaptcha_client.create_assessment(request)

        if not response.token_properties.valid:
            raise AppMessageException('recaptcha verification failed: invalid token')
        if response.token_properties.action != action:
            raise AppMessageException('recaptcha verification failed: action mismatch')
        if response.risk_analysis.score < float(settings.get('RECAPTCHA_SCORE_THRESHOLD', 0.8)):
            raise AppMessageException('recaptcha verification failed: low score')
