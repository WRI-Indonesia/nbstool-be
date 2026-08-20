from flask import Flask, g as g_var
from flask.wrappers import Response
from application.utils.logger import logging

import gc

class AppExtensions(object):

    def __init__(self, app: Flask = None):
        if app is not None:
            self.init_app(app)
    
    def init_app(self, app: Flask):

        @app.before_request
        def init_request():
            g_var.__print_list__ = []

        @app.after_request
        def after_request_callback(response):

            # if type(response) == Response and response.is_json:
            #     response.headers['Content-Type'] = '{}; {}'.format(response.headers['Content-Type'], 'charset=utf-8')

            if g_var.get('__log_it__') and g_var.get('__session_id__'):
                try:
                    logging(response)
                except Exception as e:
                    app.logger.warning('error in logging (after_request): {}'.format(str(e)))

            if g_var.__print_list__:
                app.logger.info('\n'.join(g_var.__print_list__))

            app.logger.info('garbage collected: {}'.format(gc.collect()))
            return response
        
