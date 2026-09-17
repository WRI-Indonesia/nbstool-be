import json

from flask import current_app
from google.cloud import pubsub_v1

class CloudPubSub():

    def __init__(self):
        self.publisher = pubsub_v1.PublisherClient()

    def publish(self, topic, data, **attributes):
        """topic: full path 'projects/<project>/topics/<topic>'. data: json-serializable dict. returns message id."""
        payload = json.dumps(data, default=str).encode('utf-8')
        future = self.publisher.publish(topic, payload, **attributes)
        message_id = future.result(timeout=30)
        current_app.logger.info('CloudPubSub Publish OK: {} | {}'.format(str(topic), str(message_id)))
        return message_id
