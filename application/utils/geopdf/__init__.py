# python3 application/utils/geopdf/__init__.py

from osgeo import gdal, ogr, osr
import json

import requests
import uuid
import os
import shutil

import geopandas as gpd
import matplotlib, matplotlib.pyplot as plt, contextily as cx
from matplotlib_scalebar.scalebar import ScaleBar
from geo_northarrow import add_north_arrow
from mpl_toolkits.axes_grid1.inset_locator import inset_axes
from matplotlib.patches import Polygon as poly_patches
from mpl_toolkits.basemap import Basemap
from pyproj import CRS
from shapely.geometry.point import Point
from math import cos, pi
import io

try:
    from .legend import draw_legend
except:
    from legend import draw_legend

pdf_drv = gdal.GetDriverByName('PDF')

def get_figma_y(height, v):
    return height - v

def scale_size(size: tuple, mult: float) -> tuple:
    return (size[0]*mult, size[1]*mult)

def calculate_boundaries(lat, lng, zoom, width, height): # -> tuple:
    upper_left, lower_right = {}, {}
    C = 40075 # km - Equator distance around the world
    y = pi * lat / 180 # convert latitude degree to radian
    S = C * cos(y) / 2 ** (zoom + 8) # km distance of 1 px - https://wiki.openstreetmap.org/wiki/Pt:Zoom_levels
    S_deg = S * cos(y) / 100 # convert km (distance of 1 px) to degrees (coordinates)

    upper_left['lat'] = lat + height / 2 * S_deg
    upper_left['lng'] = lng - width / 2 * S_deg

    lower_right['lat'] = lat - height / 2 * S_deg
    lower_right['lng'] = lng + width / 2 * S_deg

    return upper_left, lower_right

def rgb_scale(r, g, b):
    return (r/256, g/256, b/256)

def interpolate_color(c1, c2, t):
    """Linear interpolation between two Colors"""
    r = c1.red + (c2.red - c1.red) * t
    g = c1.green + (c2.green - c1.green) * t
    b = c1.blue + (c2.blue - c1.blue) * t
    return Color(r, g, b)

def draw_vertical_multi_gradient(c, x, y, width, height, colors, steps=500):
    segments = len(colors) - 1
    steps_per_segment = steps // segments
    
    for i in range(steps):
        seg = i // steps_per_segment
        t = (i % steps_per_segment) / steps_per_segment
        
        if seg >= segments:
            seg = segments - 1
            t = 1.0

        col = interpolate_color(colors[seg], colors[seg+1], t)
        c.setFillColor(col)
        
        # Draw one thin stripe
        stripe_height = height / steps
        c.rect(x, y + i * stripe_height, width, stripe_height, stroke=0, fill=1)

def generate_geopdf(geom:ogr.Geometry, layers:list=[], base_layer_list:list=[], geopdf_title=''):
    basepath = 'generated-geopdf'
    if not os.path.exists(basepath):
        os.makedirs(basepath)
    
    curpath = '{}/{}'.format(basepath, str(uuid.uuid4()))
    if not os.path.exists(curpath):
        os.makedirs(curpath)
    
    basexml = open('application/utils/geopdf/assets/base.xml', 'r').read()

    layers_xml_layer = ''
    layers_xml_raster = ''

    found_layers = []
    layers_name_reference = {}

    for group in base_layer_list:
        group_created = False
        group_layer = ''
        group_raster = ''
        for parent in group['items']['global']:
            parent_created = False
            parent_layer = ''
            parent_raster = ''
            if parent.get('layers'):
                if not parent.get('layers') in layers:
                    continue
                found_layers.append(parent.get('layers'))
                layers_name_reference[parent.get('layers').split(':')[-1]] = {
                    'name': parent.get('name'),
                    'group_name': group.get('title'),
                    'parent_name': '',
                }
                l = parent.get('layers').replace(':', '_')
                if not group_created:
                    group_created = True
                    print('create group: {}'.format(group.get('title')))
                print('create layers: {}'.format(l))
                group_layer += '<Layer id="{}" name="{}" initiallyVisible="true" />\n'.format(l, parent.get('name'))
                group_raster += '''<IfLayerOn layerId="{}">
                    <Raster georeferencingId="georeferenced" dataset="{}">
                        <Blending function="Normal" opacity="1.00" />
                    </Raster>
                </IfLayerOn>\n\n'''.format(l, '{}/{}.tif'.format(curpath, l))
            for child in parent['child']:
                if not child.get('gs_name') in layers:
                    continue
                found_layers.append(child.get('gs_name'))
                layers_name_reference[child.get('gs_name').split(':')[-1]] = {
                    'name': child.get('layer_name'),
                    'group_name': group.get('title'),
                    'parent_name': parent.get('name'),
                }
                l = child.get('gs_name').replace(':', '_')
                if not group_created:
                    group_created = True
                    print('create group: {}'.format(group.get('title')))
                if not parent_created:
                    parent_created = True
                    print('create parent: {}'.format(parent.get('name')))
                print('create layers: {}'.format(l))
                parent_layer += '<Layer id="{}" name="{}" initiallyVisible="true" />\n'.format(l, child.get('layer_name'))
                parent_raster += '''<IfLayerOn layerId="{}">
                    <Raster georeferencingId="georeferenced" dataset="{}">
                        <Blending function="Normal" opacity="1.00" />
                    </Raster>
                </IfLayerOn>\n\n'''.format(l, '{}/{}.tif'.format(curpath, l))
            
            if parent_layer:
                group_layer += '''<Layer id="{}" name="{}" initiallyVisible="true">
                    {}
                </Layer>\n'''.format(parent.get('name').lower().replace(' ', '_'), parent.get('name'), parent_layer)
                group_raster += '''<IfLayerOn layerId="{}">
                    {}
                </IfLayerOn>\n\n'''.format(parent.get('name').lower().replace(' ', '_'), parent_raster)
        
        if group_created:
            layers_xml_layer += '''<Layer id="{}" name="{}" initiallyVisible="true">
                {}
            </Layer>\n'''.format(group.get('title').lower().replace(' ', '_'), group.get('title'), group_layer)
            layers_xml_raster += '''<IfLayerOn layerId="{}">
                {}
            </IfLayerOn>\n\n'''.format(group.get('title').lower().replace(' ', '_'), group_raster)

    layers = found_layers # 200iq prevent injection / invalid layer

    list_json = []
    for layer in layers:
        l = layer.replace(':', '_')
        # https://nbstool.scenecoalition.org/layer/nbs/wms?SERVICE=WMS&VERSION=1.3.0&REQUEST=GetLegendGraphic&FORMAT=application%2Fjson&LAYER=nbs%3Afc_idn_2003
        url = 'https://nbstool.scenecoalition.org/layer/nbs/wms?SERVICE=WMS&VERSION=1.3.0&REQUEST=GetLegendGraphic&FORMAT=application/json&LAYER={}'
        url = url.format(layer)
        print(url)
        r = requests.get(url)
        
        fpath = '{}/{}.json'.format(curpath, l)
        open(fpath, 'wb').write(r.content)
        list_json.append(fpath)

    # draw legend
    legend_buf, add_height = draw_legend(list_json, layers_name_reference)
    open('{}/legend.png'.format(curpath), 'wb').write(legend_buf.getvalue())

    base_height = 460
    width, height = 842, base_height + add_height

    bounding_box = geom.GetEnvelope()

    longitude_kiri = bounding_box[0]
    longitude_kanan = bounding_box[1]
    latitude_bawah = bounding_box[2]
    latitude_atas = bounding_box[3]

    total_x = longitude_kanan - longitude_kiri
    total_y = latitude_atas - latitude_bawah

    base_x1 = 20
    base_x2 = width-20
    base_y1 = get_figma_y(height, 450)
    base_y2 = get_figma_y(height, 60)
    # 802, 390
    print((base_x1, base_x2, base_y1, base_y2))

    total_base_x = base_x2 - base_x1
    total_base_y = base_y2 - base_y1

    if total_x > total_y:
        latitude_atas += 0.03
        latitude_bawah -= 0.03
    else:
        longitude_kiri -= 0.03
        longitude_kanan += 0.03

    total_x = longitude_kanan - longitude_kiri
    total_y = latitude_atas - latitude_bawah

    mul_x = total_x * 10**5
    mul_y = total_y * 10**5

    sud_x = mul_y / (total_base_y / total_base_x)
    dif_x = sud_x - mul_x
    dif_x_half = dif_x / 2
    addon_x = dif_x_half / 10**5
    longitude_kiri -= addon_x
    longitude_kanan += addon_x

    zoom_mult = 1/5
    add_zoom_ratio_x = 1 * zoom_mult
    add_zoom_ratio_y = total_base_y / total_base_x * zoom_mult

    longitude_kiri -= add_zoom_ratio_x
    longitude_kanan += add_zoom_ratio_x
    latitude_atas += add_zoom_ratio_y
    latitude_bawah -= add_zoom_ratio_y

    print((longitude_kiri, latitude_bawah, longitude_kanan, latitude_atas))

    base_logo_size = (150, 55)
    logo_size = scale_size(base_logo_size, 0.75)
    logo_x1 = 20
    logo_x2 = logo_x1 + logo_size[0]
    logo_y1 = get_figma_y(height, 30) - logo_size[1]/2
    logo_y2 = logo_y1 + logo_size[1]

    logo_path = '{}/logo.vrt'.format(curpath)
    basemap_path = '{}/basemap.jpg'.format(curpath)
    inmap_path = '{}/inmap.png'.format(curpath)
    polygon_path = '{}/test.shp'.format(curpath)
    legend_path = '{}/legend.png'.format(curpath)
    labels_path = '{}/labels.csv'.format(curpath)
    print(logo_path)
    print(basemap_path)
    print(inmap_path)
    print(polygon_path)
    print(labels_path)

    xml_content = basexml.format(
        width = width,
        height = height,
        geo_x1 = base_x1,
        geo_y1 = base_y1,
        geo_x2 = base_x2,
        geo_y2 = base_y2,
        longitude_kiri = longitude_kiri,
        longitude_kanan = longitude_kanan,
        latitude_bawah = latitude_bawah,
        latitude_atas = latitude_atas,
        logo_x1 = logo_x1,
        logo_x2 = logo_x2,
        logo_y1 = logo_y1,
        logo_y2 = logo_y2,

        logo_path = logo_path,
        basemap_path = basemap_path,
        inmap_path = inmap_path,
        polygon_path = polygon_path,
        legend_path = legend_path,
        labels_path = labels_path,

        legend_x1 = 0,
        legend_y1 = 0,
        legend_x2 = width,
        legend_y2 = get_figma_y(height, base_height),
        
        layers_xml_layer = layers_xml_layer,
        layers_xml_raster = layers_xml_raster,
    )

    print(xml_content)
    
    # basemap -------------------------------------------------------------------------------
    geom_centroid = geom.Centroid()
    cm = 0.0264583333 # 1 pixels = 0.0264583333
    fig, ax = plt.subplots(figsize=(total_base_x*cm, total_base_y*cm))
    ax.set_xlim([longitude_kiri, longitude_kanan])
    ax.set_ylim([latitude_bawah, latitude_atas])
    # geom_plot = loc.plot(ax=ax, facecolor='none', edgecolor="#077f68", linewidth=3)
    cx.add_basemap(
        ax,
        crs=CRS('EPSG:4326'),
        source=cx.providers.OpenStreetMap.Mapnik,
        attribution="",
    )

    # basemap
    ax.set_axis_off()
    fig.tight_layout(pad=0)
    fig.savefig(basemap_path, bbox_inches="tight", pad_inches=0)
    # end basemap -------------------------------------------------------------------------------

    # inmap attribution -------------------------------------------------------------------------------
    fig, ax = plt.subplots(figsize=(total_base_x*cm, total_base_y*cm))
    ax.set_xlim([longitude_kiri, longitude_kanan])
    ax.set_ylim([latitude_bawah, latitude_atas])

    points = gpd.GeoSeries([Point(longitude_kiri, latitude_bawah), Point(longitude_kanan, latitude_atas)], crs=4326)
    points = points.to_crs(32619)
    distance_meters = points[0].distance(points[1])
    scalebar = ScaleBar(distance_meters, location="lower left", border_pad=1, pad=0.5, label="Map Scale", scale_loc="right") # 1 pixel = 0.2 meter
    ax.add_artist(scalebar)
    add_north_arrow(ax, scale=.25, xlim_pos=.98, ylim_pos=.94, color='#000', text_scaler=4, text_yT=-2.5)

    axin = inset_axes(ax, width="20%", height="20%", loc="lower right")
    upper_left, lower_right = calculate_boundaries(geom_centroid.GetY(), geom_centroid.GetX(), 7, total_base_x*cm*100, total_base_y*cm*100)
    upper_left_inmap, lower_right_inmap = calculate_boundaries(geom_centroid.GetY(), geom_centroid.GetX(), 10, total_base_x*cm*100, total_base_y*cm*100)
    print(upper_left)
    print(lower_right)
    inmap = Basemap(llcrnrlon=upper_left['lng'], urcrnrlon=lower_right['lng'], llcrnrlat=lower_right['lat'], urcrnrlat=upper_left['lat'], projection='lcc', lon_0=geom_centroid.GetX(), lat_0=geom_centroid.GetY(), resolution='c', ax=axin)
    inmap.shadedrelief(scale=.5)
    inmap_lngs = [upper_left_inmap['lng'], upper_left_inmap['lng'], lower_right_inmap['lng'], lower_right_inmap['lng']]
    inmap_lats = [lower_right_inmap['lat'], upper_left_inmap['lat'], upper_left_inmap['lat'], lower_right_inmap['lat']]
    x, y = inmap(inmap_lngs, inmap_lats)
    xy = zip(x,y)
    poly = poly_patches( list(xy), facecolor='red', alpha=0.4, closed=False )
    inmap.ax.add_patch(poly)

    # inmap attribution
    ax.set_axis_off()
    fig.tight_layout(pad=0)
    fig.savefig(inmap_path, bbox_inches="tight", pad_inches=0, transparent=True)
    # end inmap attribution -------------------------------------------------------------------------------

    # open logo
    gdal.Translate(logo_path, gdal.Open('application/utils/geopdf/assets/scene-black.logo.png', gdal.GA_ReadOnly), format="VRT")

    # geom polygon
    ds = ogr.GetDriverByName("ESRI Shapefile").CreateDataSource(polygon_path)
    srs = osr.SpatialReference()
    srs.ImportFromEPSG(4326)
    lyr = ds.CreateLayer("test", srs=srs)
    f = ogr.Feature(lyr.GetLayerDefn())
    f.SetGeometryDirectly(ogr.CreateGeometryFromWkt(geom.ExportToWkt()))
    lyr.CreateFeature(f)
    ds = None

    # layers
    for layer in layers:
        url = 'https://gis.scenecoalition.org/geoserver/nbs/wms?service=WMS&version=1.1.0&request=GetMap&layers={}&bbox={},{},{},{}&width={}&height={}&srs=EPSG:4326&styles=&format=image/geotiff'
        url = url.format(layer, longitude_kiri, latitude_bawah, longitude_kanan, latitude_atas, width, height)
        print(url)
        r = requests.get(url)

        l = layer.replace(':', '_')
        open('{}/{}.tif'.format(curpath, l), 'wb').write(r.content)

    # label
    x_title = width-120
    y_title = height-25
    csv_text = '''id,WKT,OGR_STYLE
1,"POINT({x_title} {y_title})","LABEL(f:""Helvetica"",t:""Interactive"",s:50pt,bo:1,it:1)"
2,"POINT({x_title_2} {y_title_2})","LABEL(f:""Helvetica"",t:""GeoPDF"",s:50pt,bo:1,it:1)"'''

    limit_len_text = 40
    if len(geopdf_title) <= limit_len_text:
        csv_text += chr(10)
        csv_text += '3,"POINT({} {})","LABEL(f:""Helvetica"",t:""{}"",s:50pt,bo:1,p:5)"'.format('{x_title_3}', '{y_title_3}', geopdf_title)
    else:
        split_title = geopdf_title.split(' ')
        
        geopdf_titles = []
        tmp = ''
        for x in split_title:
            if len((tmp + ' ' + x).strip()) > limit_len_text:
                geopdf_titles.append(tmp.strip())
                tmp = ''
            
            tmp += ' ' + x
        
        geopdf_titles.append(tmp)

        idx = 3
        for x in geopdf_titles:
            csv_text += chr(10) + '''{},"POINT({} {})","LABEL(f:""Helvetica"",t:""{}"",s:50pt,bo:1,p:5)"'''.format(idx, '{x_title_3}', y_title-20*(idx-3)+5, x)
            idx += 1

    open('{}/labels.csv'.format(curpath), 'w').write(csv_text.format(
        x_title = x_title,
        y_title = y_title,
        x_title_2 = x_title+17,
        y_title_2 = y_title-20,
        x_title_3 = width/2,
        y_title_3 = y_title-5,
    ))

    out_path = "{}/geo.pdf".format(curpath)
    out_ds = pdf_drv.Create(
        out_path, 0, 0, 0,
        gdal.GDT_Unknown,
        options=["COMPOSITION_FILE=" + xml_content],
    )

    buf = open(out_path, 'rb')

    shutil.rmtree(curpath)

    return buf

def main():
    dataset = ogr.Open('/home/del/Downloads/muara_merang_4326/muara_merang_4326.shp')
    layer = dataset.GetLayer()
    feature = layer[0]
    geom = feature.GetGeometryRef()

    base_layer_list = json.loads(open('/home/del/Downloads/v2.json', 'r').read())['result']
    layers = [
        'nbs:adm_boundaries',
        'nbs:current_land_cover',
        'nbs:tree_species_richness',
        'nbs:annual_mean_temperature',
        'nbs:peatland_50k',
        'nbs:social_forestry_permit',
        'nbs:fc_idn_2003',
    ]
    open('/home/del/Downloads/geo.pdf', 'wb').write(generate_geopdf(geom, layers, base_layer_list, 'Test Title GeoPDF Test Title GeoPDF Test Title GeoPDF').read())

if __name__ == '__main__':
    main()