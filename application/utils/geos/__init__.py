# application/utils/geos/__init__.py
from flask import current_app
from ... import db

from sqlalchemy import create_engine
from sqlalchemy.orm import scoped_session, sessionmaker

import os
import geopandas
import shutil
import pathlib
import matplotlib
import matplotlib.pyplot as plt
import numpy as np

from shutil import copy2
from shapely.geometry import shape

from ..cloud_storage import CloudStorage

# GisDbSession = scoped_session(sessionmaker(bind=db.engines['gis']))
gcs = CloudStorage()

class GeoUtils():

    GIS_DB_CONNECTION_TIMEOUT = 10000 # 10 sec
    engine = create_engine(current_app.config.get('GIS_DB_CONSTRING'), pool_size=30, max_overflow=0) # as recommended in https://docs.sqlalchemy.org/en/20/core/pooling.html
    connection = engine.connect()
    connection.execute(db.text("SET statement_timeout = {}".format(GIS_DB_CONNECTION_TIMEOUT)))
    current_app.logger.info('''
    -------------------------------------------------------------------------------------------------
    GEOUTILS
    -------------------------------------------------------------------------------------------------
    ''')

    @staticmethod
    def refresh_connection():
        current_app.logger.info('refresh connection...')
        try:
            GeoUtils.connection.invalidate()
        except Exception as e:
            current_app.logger.info('failed invalidate gis db connection')

        try:
            GeoUtils.connection.close()
        except Exception:
            current_app.logger.info('failed close gis db connection')

        GeoUtils.connection = GeoUtils.engine.connect()
        GeoUtils.connection.execute(db.text("SET statement_timeout = {}".format(GeoUtils.GIS_DB_CONNECTION_TIMEOUT)))
        current_app.logger.info('done refresh connection...')

    @staticmethod
    def get_db(query_text:db.text, gis_db:bool=True) -> dict:
        output = []

        if gis_db:
            try:
                result = GeoUtils.connection.execute(query_text)
                # connection.close()
                # engine.dispose()
            except Exception as e:
                current_app.logger.info('error while execute gis db query: {}'.format(str(e)))
                GeoUtils.refresh_connection()
                result = GeoUtils.connection.execute(query_text)
        else:
            result = db.session.execute(query_text).all()

        for row in result:
            output.append(row._asdict())

        return output
    
    @staticmethod
    def get_db_function_data(func:str, col:list, params:str, gis_db:bool=False, multirow:bool=False) -> list:
        if multirow:
            data = list()
        else:
            data = dict()
        
        # conditions = []
        # for cond in params.keys():
        #     conditions.append('{} = \'{'.format(cond) + '\'}')

        query = """select {select_col} from public."{func}"('{params}')""".format(func=func, select_col=','.join(col), params=params)
        dt = GeoUtils.get_db(db.text(query), gis_db=gis_db)

        for row in dt:
            for c in col:
                data[c] = row.get(c)
        
        return data

    @staticmethod
    def construct_gdf(data):
        # karena permintaan deddy, maka ada perubahan construction di gdf bawaan geometry python
        
        construction_type = data['geometry']['type']
        new_data = data['geometry']
        if construction_type.lower() == "polygon":
            construction_coordinate = data['geometry']['coordinates'][0]
        
            new_data = {"type": construction_type,
                    "coordinates": [construction_coordinate]}
        
        polygon = shape(new_data)
        
        gdf4326 = geopandas.GeoDataFrame(index=[0], crs='epsg:4326', geometry=[polygon])
        gdf54034 = gdf4326.to_crs({'proj':'cea'})
        gdf54034['hectare'] = gdf54034.geometry.area / 10000

        print("The area of your drawn polygon is ", round(gdf54034.iloc[0]['hectare']), " hectare")

        return gdf54034
    

    @staticmethod
    def allowed_file(filename):
        ALLOWED_EXTENSIONS = {'kml', 'kmz', 'zip'}
        return filename != '' and '.' in filename and filename.rsplit('.', 1)[1].lower() in ALLOWED_EXTENSIONS
    

    @staticmethod
    def user_remove_upload_tree_file(*paths):
        user_folder = pathlib.Path(*paths).resolve()
        if os.path.isdir(user_folder):
            shutil.rmtree(user_folder)
    

    @staticmethod
    def remove_process_folder(session_id: str, section: str):
        user_folder = pathlib.Path(temp_file_path, session_id, section).resolve()

        if os.path.isdir(user_folder):
            shutil.rmtree(user_folder)
    

    @staticmethod
    def join_and(list, postfix = " and "):
        list_count = len(list)

        result = ""
        if list_count > 2:
            result = ', '.join(list[:-1]) + "," + postfix + " " + str(list[-1])
        elif list_count == 2:
            result = postfix.join(list)
        elif list_count == 1:
            result = list[0]
        
        return result


    @staticmethod
    def create_graph(session_id, section, data, categories, color="#0000FF"):
        matplotlib.use('agg')

        plt.cla()
        plt.clf()

        height = data
        bars = categories
        y_pos = np.arange(len(bars))

        # Create bars
        plt.bar(y_pos, height, color = color)

        # Create names on the x-axis
        plt.xticks(y_pos, bars)

        # Save graphic
        base_path = pathlib.Path("generated-file", "graph").resolve()
        filename = session_id + "_" + section + "_graph.jpg"
        filepath = pathlib.Path("generated-file", "graph",  filename).resolve()
        if not os.path.exists(base_path):
            os.makedirs(base_path)

        plt.savefig(filepath)

        gcs.upload(os.path.join('generated-file', 'graph', filename))
    
    @staticmethod
    def string_to_list(s:str, delim:str) -> list:
        return s.split(delim) if s else list()