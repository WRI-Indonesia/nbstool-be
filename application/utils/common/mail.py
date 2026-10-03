from flask import render_template, current_app
# off: brevo, replaced by google workspace smtp
# import sib_api_v3_sdk
# from sib_api_v3_sdk.rest import ApiException
from email.mime.text import MIMEText
from email.utils import formataddr
import smtplib
import os

from ...models.master_models.models import Settings

# configuration = sib_api_v3_sdk.Configuration()
# configuration.api_key['api-key'] = os.environ.get('MAIL_BREVO_API_KEY')
#
# api_instance = sib_api_v3_sdk.TransactionalEmailsApi(sib_api_v3_sdk.ApiClient(configuration))

class BaseMail():
    def __init__(self, to, subject='No Reply', template='base.html', data={}):
        self.sender = {"name": os.environ.get('MAIL_SMTP_FROM_NAME'), "email": os.environ.get('MAIL_SMTP_USER')}
        self.to = to
        self.subject = subject
        self.template = template

        FE_URL = Settings.find_by_name('FE_URL_V3')
        try:
            FE_URL = FE_URL.value
        except Exception as e:
            current_app.logger.info('utils common mail: {}'.format(str(e)))
            raise Exception('utils common mail: fe url v3 invalid or not found')

        self.data = {
            'fe_url': FE_URL
        }
        self.data.update(data)
    
    def send_mail(self):
        to = self.to.split(';')

        # debug
        current_app.logger.info(to)
        current_app.logger.info('---------------------------')
        current_app.logger.info(self.data)
        current_app.logger.info('---------------------------')
        current_app.logger.info(render_template(self.template, data=self.data))
        # end debug

        message = MIMEText(render_template(self.template, data=self.data), 'html', 'utf-8')
        message['Subject'] = self.subject
        message['From'] = formataddr((self.sender['name'], self.sender['email']))
        message['To'] = ', '.join(to)

        host = os.environ.get('MAIL_SMTP_HOST')
        port = int(os.environ.get('MAIL_SMTP_PORT') or 587)

        try:
            # 465 = implicit TLS, anything else (587) = STARTTLS
            if port == 465:
                server = smtplib.SMTP_SSL(host, port, timeout=30)
            else:
                server = smtplib.SMTP(host, port, timeout=30)
                server.starttls()
            with server:
                server.login(os.environ.get('MAIL_SMTP_USER'), os.environ.get('MAIL_SMTP_PASSWORD'))
                server.sendmail(self.sender['email'], to, message.as_string())
            return True
        except Exception as e:
            current_app.logger.info("Exception when sending smtp mail: %s" % e)
            return False

    # callers still use the brevo-era name
    send_brevo_mail = send_mail


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


class EMailProjectDeletionReminder():
    SUBJECT = '[Action needed] "{}" will be deleted' # .format(project_name)
    TEMPLATE = 'project_deletion_reminder.html'
