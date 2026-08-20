from flask import render_template, current_app
import sib_api_v3_sdk
from sib_api_v3_sdk.rest import ApiException
import os

from ...models.master_models.models import Settings

configuration = sib_api_v3_sdk.Configuration()
configuration.api_key['api-key'] = os.environ.get('MAIL_BREVO_API_KEY')

api_instance = sib_api_v3_sdk.TransactionalEmailsApi(sib_api_v3_sdk.ApiClient(configuration))

class BaseMail():
    def __init__(self, to, subject='No Reply', template='base.html', data={}):
        self.sender = {"name":"SCeNe Coalition","email":"info@scenecoalition.com"}
        self.to = to
        self.subject = subject
        self.template = template

        FE_URL = Settings.find_by_name('FE_URL')
        try:
            FE_URL = FE_URL.value
        except Exception as e:
            current_app.logger.info('utils common mail: {}'.format(str(e)))
            raise Exception('utils common mail: fe url invalid or not found')

        self.data = {
            'fe_url': FE_URL
        }
        self.data.update(data)
    
    def send_brevo_mail(self):
        if ';' in self.to:
            to = [{ 'email': n } for n in self.to.split(';')]
        else:
            to = [{ "email": self.to }]
        
        # params = {"parameter":"My param value","subject":"New Subject"}
        # send_smtp_email = sib_api_v3_sdk.SendSmtpEmail(to=to, bcc=bcc, cc=cc, reply_to=reply_to, headers=headers, html_content=html_content, sender=sender, subject=subject)

        # debug
        current_app.logger.info(to)
        current_app.logger.info('---------------------------')
        current_app.logger.info(self.data)
        current_app.logger.info('---------------------------')
        current_app.logger.info(render_template(self.template, data=self.data))
        # return True
        # end debug
        
        try:
            send_smtp_email = sib_api_v3_sdk.SendSmtpEmail(
                to=to, 
                html_content=render_template(self.template, data=self.data), 
                sender=self.sender, 
                subject=self.subject
            )

            api_response = api_instance.send_transac_email(send_smtp_email)
            current_app.logger.info(api_response)
            return True
        except ApiException as e:
            current_app.logger.info("Exception when calling SMTPApi->send_transac_email: %s\n" % e)
            return False


# enums
class EMailUserRegister(): # enum mail
    SUBJECT = 'Welcome to NbS Tool! Activate your account and unlock your potential'
    TEMPLATE = 'user_register.html'


class EMailUserForgotPassword():
    SUBJECT = 'NbS Tool Password Reset: Get Back on Track'
    TEMPLATE = 'forgot_password.html'


class EMailFeasibilityDocument():
    SUBJECT = 'NbS Tool: Your Feasibility Document is Ready!'
    TEMPLATE = 'feasibility_document.html'


class EMailIncubatorsReviewToIncubator():
    SUBJECT = '[NbS Tool] Review Request for User\'s Project Document'
    TEMPLATE = 'document_review_incubator.html'


class EMailIncubatorsReviewToUser():
    SUBJECT = '[NbS Tool] Project Document Review Submission Confirmation'
    TEMPLATE = 'document_review_user.html'


class EMailReviewUserRequest():
    SUBJECT = '[NbS Tool] Request for Broader Area ANalysis'
    TEMPLATE = 'user_area_request.html'