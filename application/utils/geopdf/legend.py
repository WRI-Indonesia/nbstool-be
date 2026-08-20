import math
import json
import io
import gc

from reportlab.pdfgen import canvas as cvs
from reportlab.lib.colors import Color
from reportlab.pdfbase.pdfmetrics import stringWidth

from pdf2image import convert_from_bytes

from datetime import datetime, UTC

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

def add_text(canvas, fontStyle="Helvetica", fontSize=5, x=0, y=0, s="test", charSpace=0):
    text = canvas.beginText(x, y)
    if charSpace:
        text.setCharSpace(charSpace)
    text.setFont(fontStyle, fontSize)
    text.textLine(s)
    canvas.drawText(text)

def check_width(fontStyle, fontSize, s):
    return stringWidth(s, fontStyle, fontSize)

def figma_y(page_height, y, h): # reverse y
    return page_height - y - h

def stroke_legend(canvas, name, name_ref, legend, page_width, page_height, init_x=9, init_y=0):
    init_y = page_height - init_y

    max_x = init_x
    
    font_size = 13
    h_label = font_size

    x = init_x
    y = init_y - h_label

    label = '{}'.format(name)
    w_label = check_width('Helvetica-Bold', font_size, label)
    if w_label > max_x:
        max_x = w_label
    add_text(canvas, 'Helvetica-Bold', font_size, x, y, label)

    y -= font_size
    font_size = 9
    h_label = font_size
    label = name_ref.get('group_name') if name_ref else ''
    label += ' - {}'.format(name_ref.get('parent_name')) if label and name_ref.get('parent_name') else ''
    w_label = check_width('Helvetica', font_size, label)
    if w_label > max_x:
        max_x = w_label
    add_text(canvas, 'Helvetica', font_size, x, y, label)

    y -= 5 # gap header

    x_text = init_x+16
    x_val = x_text-12.5
    font_size = 12
    h_label = 16
    circle_size = 5

    gcolor = []
    is_ramp = False
    y_first_item = y
    for item in legend:
        # print('name: "{}" | fill: "{}" | type: "{}"'.format(item.get('name'), item.get('fill'), item.get('type')))
        fill = item.get('fill') if item.get('fill') else '#ffffff'
        gcolor.append(Color(*rgb_scale(*tuple(int(fill[i:i+2], 16) for i in (1, 3, 5)))))

        y -= h_label
        label = '{}'.format(item.get('name') if item.get('name') else '').replace('⁰', '')
        w_label = check_width('Helvetica', font_size, label)
        if w_label > max_x:
            max_x = w_label
        add_text(canvas, 'Helvetica', font_size, x_text, y, label)

        if item.get('type') == 'value' or item.get('type') == 'values':
            canvas.setFillColorRGB(*rgb_scale(*tuple(int(fill[i:i+2], 16) for i in (1, 3, 5))))
            canvas.circle(x_val+circle_size/2, y+font_size/3, circle_size, stroke=1, fill= 1)
            canvas.setFillColorRGB(0, 0, 0)
        elif item.get('type') == 'ramp':
            is_ramp = True
    
    if is_ramp:
        rect_box = (x_val-2, y_first_item-h_label*len(legend)-1, circle_size+5, h_label*len(legend)-5)
        draw_vertical_multi_gradient(canvas, *rect_box, gcolor[::-1], steps=500)
        canvas.rect(*rect_box, stroke=1, fill=0)
    
    # reset black
    canvas.setFillColorRGB(0, 0, 0)

    return max_x, page_height - y

def draw_legend(list_json=[], layers_name_reference={}):
    legend_layers = get_legend(list_json)

    page_width, page_height = 842, 595

    pdf_binary = io.BytesIO()
    canvas = cvs.Canvas(pdf_binary, pagesize=(page_width, page_height))
    # canvas = cvs.Canvas('/home/del/Downloads/gas.pdf', pagesize=(page_width, page_height))

    legend_meta = {
        'max_x': 0,
        'max_y': 0,
        'total_y': 0,
        'legend_detail': []
    }

    for i, key in enumerate(legend_layers.keys()):
        legend = legend_layers[key]

        # print('---------------------------------------------------------------------------------------------- {}'.format(key))
        name_ref = layers_name_reference.get(key)
        max_x, total_y = stroke_legend(canvas, name_ref.get('name') if name_ref else key, name_ref, legend, page_width, page_height)

        max_x += 10 # pad

        if max_x > legend_meta['max_x']:
            legend_meta['max_x'] = max_x
        
        if total_y > legend_meta['max_y']:
            legend_meta['max_y'] = total_y
        
        legend_meta['total_y'] += total_y
        legend_meta['legend_detail'].append({ 'key': key, 'max_x': max_x, 'total_y': total_y })

        canvas.showPage()

        # print('finish {} | total_x: {} | total_y: {}'.format(key, max_x, total_y))

    canvas.save()

    # print(legend_meta)

    padding = 30
    minmax_x = 30
    n_columns = math.floor((page_width-minmax_x*2)/legend_meta['max_x'])
    column_width = page_width / n_columns
    padding_x = ((page_width-minmax_x*2)-n_columns*legend_meta['max_x'])/(n_columns+1)
    print(n_columns, padding_x, legend_meta)
    
    # Create columns
    columns = [{'x': i * column_width, 'y': 0, 'stack_height': 0} for i in range(n_columns)]

    # Sort the boxes by height (shorter boxes first)
    legends = sorted(legend_meta['legend_detail'], key=lambda l: l['total_y'])

    positions = []

    # Assign legends into columns left-to-right (cyclic)
    for i, legend in enumerate(legends):
        col = columns[i % n_columns]

        positions.append({
            'key': legend['key'],
            'x': col['x']+minmax_x,
            'y': col['stack_height'],
            'width': legend['max_x'],
            'height': legend['total_y']
        })

        col['stack_height'] += legend['total_y'] + padding
    
    print(positions)
    total_height_used = max(col['stack_height'] for col in columns)
    # print(f"Total height used: {total_height_used:.2f}")

    add_height = 55 # for title
    page_height = total_height_used+padding+add_height+10
    canvas = cvs.Canvas(pdf_binary, pagesize=(page_width, page_height))

    font_size = 16
    h_label = font_size
    y_bot = padding+10-add_height
    y = figma_y(page_height, 10, h_label)
    x = minmax_x

    # title
    label = 'Legend'
    w_label = check_width('Helvetica-BoldOblique', font_size, label)
    add_text(canvas, 'Helvetica-BoldOblique', font_size, x, y, label) # center: page_width/2-w_label/2

    y_bot -= h_label
    y -= h_label

    canvas.rect(x, y, page_width-minmax_x*2, 0.1)
    
    for item in positions:
        key = item['key']
        # print(f"{key} -> x: {item['x']:.2f}, y: {item['y']:.2f}, w: {item['width']:.2f}, h: {item['height']:.2f}")
        legend = legend_layers[key]

        name_ref = layers_name_reference.get(key)
        stroke_legend(canvas, name_ref.get('name') if name_ref else key, name_ref, legend, page_width, page_height, init_x=item['x'], init_y=item['y']+padding-5-y_bot)

    y = figma_y(page_height, total_height_used+add_height-10, 0)
    canvas.rect(x, y, page_width-minmax_x*2, 0.1)

    y -= 10

    font_size = 6
    label = '{} UTC - https://nbstool.scenecoalition.org/interactive-map'.format(datetime.now(UTC).isoformat().split('.')[0].replace('T', ' '))
    w_label = check_width('Helvetica-Oblique', font_size, label)
    add_text(canvas, 'Helvetica-Oblique', font_size, x, y, label)

    canvas.save()
    pdf_binary.seek(0)
    cimg = convert_from_bytes(pdf_binary.read())
    pdf_binary = ''
    gc.collect()
    img_binary = io.BytesIO()
    cimg[0].save(img_binary, 'PNG')
    cimg = ''
    gc.collect()

    return img_binary, page_height

def get_symbolizer_type(symbolizer):
    return next(iter(symbolizer.keys()), None)  # Returns 'Polygon', 'Line', etc

def get_legend(list_json=[]):
    if not list_json:
        list_json = [
            '/home/del/Documents/WRI-DEV/nbs_be/generated-geopdf/7b1c9f18-bc58-4b4f-bbd5-d648f1d5b978/nbs_adm_boundaries.json',
            '/home/del/Documents/WRI-DEV/nbs_be/generated-geopdf/7b1c9f18-bc58-4b4f-bbd5-d648f1d5b978/nbs_annual_mean_temperature.json',
            '/home/del/Documents/WRI-DEV/nbs_be/generated-geopdf/7b1c9f18-bc58-4b4f-bbd5-d648f1d5b978/nbs_current_land_cover.json',
            '/home/del/Documents/WRI-DEV/nbs_be/generated-geopdf/7b1c9f18-bc58-4b4f-bbd5-d648f1d5b978/nbs_fc_idn_2003.json',
            '/home/del/Documents/WRI-DEV/nbs_be/generated-geopdf/7b1c9f18-bc58-4b4f-bbd5-d648f1d5b978/nbs_peatland_50k.json',
            '/home/del/Documents/WRI-DEV/nbs_be/generated-geopdf/7b1c9f18-bc58-4b4f-bbd5-d648f1d5b978/nbs_social_forestry_permit.json',
            '/home/del/Documents/WRI-DEV/nbs_be/generated-geopdf/7b1c9f18-bc58-4b4f-bbd5-d648f1d5b978/nbs_tree_species_richness.json',
            # '/home/del/Documents/WRI-DEV/nbs_be/generated-geopdf/7b1c9f18-bc58-4b4f-bbd5-d648f1d5b978/mining_con.json',
        ]

    legend_layers = {}

    for path in list_json:
        data = json.loads(open(path, 'r').read())
        
        # print('-----------------------------------------------')
        # print(data['Legend'][0]['layerName'])
        # print(data['Legend'][0]['title'])

        for layer in data['Legend']:
            for rule in layer.get('rules', []):
                for symbolizer in rule.get('symbolizers', []):
                    symbolizer_type = get_symbolizer_type(symbolizer)
                    symbolizer = symbolizer.get(symbolizer_type)

                    if symbolizer_type == 'Polygon':
                        if not symbolizer.get('fill'):
                            continue
                        if legend_layers.get(layer['layerName']) is None:
                            legend_layers[layer['layerName']] = []
                        legend_layers[layer['layerName']].append({
                            'name': rule.get('name') if rule.get('name') else rule.get('title'),
                            'fill': symbolizer.get('fill'),
                            'fill_opacity': symbolizer.get('fill-opacity'),
                            'type': 'value',
                        })
                        # print(rule.get('name') if rule.get('name') else rule.get('title'), symbolizer.get('fill'))
                    elif symbolizer_type == 'Raster':
                        for entry in symbolizer.get("colormap", {}).get("entries", []):
                            if legend_layers.get(layer['layerName']) is None:
                                legend_layers[layer['layerName']] = []
                            legend_layers[layer['layerName']].append({
                                'name': entry.get('label'),
                                'fill': entry.get('color'),
                                'type': symbolizer.get("colormap", {}).get('type'),
                            })
                            # print(entry.get('label'), entry.get('color'))
                    elif symbolizer_type == 'Line':
                        if legend_layers.get(layer['layerName']) is None:
                            legend_layers[layer['layerName']] = []
                        legend_layers[layer['layerName']].append({
                            'name': rule.get('name') if rule.get('name') else rule.get('title'),
                            'fill': symbolizer.get('stroke'),
                            'fill_opacity': symbolizer.get('stroke-opacity'),
                            'type': 'value',
                        })
                        # print(rule.get('name') if rule.get('name') else rule.get('title'), symbolizer.get('stroke'))
        
    # for x in legend_layers.keys():
    #     legend = legend_layers[x]
    #     print('-----------------------------------------------')
    #     for y in legend:
    #         print('name: "{}" | fill: "{}"'.format(y.get('name'), y.get('fill')))
    
    return legend_layers

def main():
    layers_name_reference = {
        'peatland_50k': { 'name': 'Indonesia Peat thickness', 'group_name': 'Country Specific', 'parent_name': 'Peatland' },
        'social_forestry_permit': { 'name': 'Indonesia', 'group_name': 'Country Specific', 'parent_name': 'Social Forestry Permit' },
        'fc_idn_2003': { 'name': 'Indonesia (2003)', 'group_name': 'Country Specific', 'parent_name': 'Forest Cover' },
        'adm_boundaries': { 'name': 'Administrative boundaries', 'group_name': 'Site Information', 'parent_name': '' },
        'current_land_cover': { 'name': 'Current land cover', 'group_name': 'Site Information', 'parent_name': '' },
        'tree_species_richness': { 'name': 'Endangered trees species richness', 'group_name': 'Nature', 'parent_name': '' },
        'annual_mean_temperature': { 'name': 'Annual mean temperature', 'group_name': 'Climate', 'parent_name': '' },
    }
    legend_buf, add_height = draw_legend(layers_name_reference=layers_name_reference)
    open('/home/del/Downloads/legend.png', 'wb').write(legend_buf.getvalue())

if __name__ == '__main__':
    main()