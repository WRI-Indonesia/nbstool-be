from sqlalchemy import text
from .. import GeoUtils

class Social():
    
    def __init__(self, location:dict):
        self.location = location
    
    def get_social_country_specific(self) -> dict:
        return {
            'demography': {
                'general': self.get_people_cs_demography_general(),
                'age_group': self.get_people_cs_demography_age_group(),
            },
            'employment': {
                'general': self.get_people_cs_employment_general(),
                'status': self.get_people_cs_employment_status(),
            },
            'education': self.get_people_cs_education_general(),
            'economy': self.get_people_cs_social_economy(),
            'health': self.get_people_cs_social_health(),
            'water': self.get_people_cs_social_water(),
        }

    def get_people_cs_demography_general(self) -> dict:
        demography_general = dict()

        select_col = ['population', 'household', 'density', 'growth', 'male', 'male_pct', 'female', 'female_pct']

        query = """select {select_col} from "vwIDN_Demography_General" where district = '{district}'"""
        query = query.format(district=self.location.get('district'), select_col=','.join(select_col))

        dt = GeoUtils.get_db(text(query), gis_db=False)

        for row in dt:
            for col in select_col:
                demography_general[col] = row.get(col)
        
        return demography_general

    def get_people_cs_demography_age_group(self) -> list:
        demography_age_group = []

        query = """select * from "vwIDN_Demography_AgeGroup" where district = '{district}'"""
        query = query.format(district=self.location.get('district'))

        dt = GeoUtils.get_db(text(query), gis_db=False)

        for row in dt:
            d = {
                'age_category': row.get('age_category'),
                'age_group': row.get('age_group'),
            }
            try:
                d['population'] = int(row.get('population'))
            except Exception as e:
                d['population'] = 0
            
            demography_age_group.append(d)
        
        return demography_age_group

    def get_people_cs_employment_general(self) -> dict:
        employment_general = dict()

        select_col = ['population', 'unemployment_rate', 'unemployment_total', 'underemployment_rate', 'underemployment_total', 'major_sector', 'major_status']

        query = """select {select_col} from "vwIDN_Employment_General" where province = '{province}'"""
        query = query.format(province=self.location.get('province'), select_col=','.join(select_col))

        dt = GeoUtils.get_db(text(query), gis_db=False)

        for row in dt:
            for col in select_col:
                employment_general[col] = row.get(col)
        
        return employment_general

    def get_people_cs_employment_status(self) -> list:
        employment_status = []

        query = """select * from "vwIDN_Employment_Status" where province = '{province}'"""
        query = query.format(province=self.location.get('province'))

        dt = GeoUtils.get_db(text(query), gis_db=False)

        for row in dt:
            employment_status.append({
                'category': row.get('category'),
                'total': row.get('total'),
            })
        
        return employment_status

    def get_people_cs_education_general(self) -> dict:
        education_general = dict()

        query = """select * from "vwIDN_Education_General" where district = '{district}';"""
        query = query.format(district=self.location.get('district'))

        dt = GeoUtils.get_db(text(query), gis_db=False)

        for row in dt:
            education_general = {
                'total_school': row.get('total_school'),
                'total_university': row.get('total_university'),
                'total_literacy': row.get('total_literacy'),
            }
        
        return education_general

    def get_people_cs_social_economy(self) -> dict:
        social_economy = dict()

        query = """select * from public."v2_get_social_economy"('{district}')"""
        query = query.format(district=self.location.get('district'))

        dt = GeoUtils.get_db(text(query), gis_db=False)

        for row in dt:
            social_economy = {
                'latest_year': row.get('latest_year'),
                'net_income_average': row.get('net_income_average'),
                'current_price': row.get('current_price'),
                'gdp_capita': row.get('gdp_capita'),
                'growth_rate': row.get('growth_rate'),
                'purchasing_power': row.get('purchasing_power'),
                'gini_ratio': row.get('gini_ratio'),
                'consumer_price': row.get('consumer_price'),
                'poverty_line': row.get('poverty_line'),
            }

            ni_sectors = row.get('net_income_sectors').split(';')
            ni_values = row.get('net_income_values').split(';')
            social_economy['net_income_sectors'] = []
            for i, sector in enumerate(ni_sectors):
                social_economy['net_income_sectors'].append({ 'sector': sector, 'value': ni_values[i] })
        
        query = """select * from public."v2_get_social_economy_industries"('{district}')"""
        query = query.format(district=self.location.get('district'))

        dt = GeoUtils.get_db(text(query), gis_db=False)

        social_economy['industries'] = []
        for row in dt:
            social_economy['industries'].append({ 'industry': row.get('industry_name'), 'value': row.get('industry_value') })
        
        query = """select * from public."v2_get_social_economy_chart"('{district}')"""
        query = query.format(district=self.location.get('district'))

        dt = GeoUtils.get_db(text(query), gis_db=False)

        for row in dt:
            social_economy['current_price_chart'] = []

            if not row.get('gdp_values'):
                continue

            gdp_values = row.get('gdp_values').split(';')
            poverty_values = row.get('poverty_values').split(';')
            income_values = row.get('income_values').split(';')
            gini_values = row.get('gini_values').split(';')
            years = row.get('years').split(';')

            for i, gdp in enumerate(gdp_values):
                social_economy['current_price_chart'].append({
                    'gdp': gdp,
                    'poverty': poverty_values[i],
                    'income': income_values[i],
                    'gini': gini_values[i],
                    'years': years[i],
                })
        
        return social_economy

    def get_people_cs_social_health(self) -> dict:
        social_health = dict()

        query = """select * from public."v2_get_social_health"('{district}')"""
        query = query.format(district=self.location.get('district'))

        dt = GeoUtils.get_db(text(query), gis_db=False)

        for row in dt:
            social_health = {
                'total_doctor': row.get('total_doctor'),
                'total_medical_worker': row.get('total_medical_worker'),
                'stunting_pct': row.get('stunting_pct'),
                'wasting_pct': row.get('wasting_pct'),
                'underweight_pct': row.get('underweight_pct'),
                'overweight_pct': row.get('overweight_pct'),
            }

            disease_type = row.get('disease_type').split(';')
            disease_percentage = row.get('disease_percentage').split(';')
            social_health['disease'] = []
            for i, disease in enumerate(disease_type):
                social_health['disease'].append({ 'disease': disease, 'percentage': disease_percentage[i] })
        
        return social_health

    def get_people_cs_social_water(self) -> dict:
        social_water = dict()

        query = """select * from public."v2_get_social_water"('{district}')"""
        query = query.format(district=self.location.get('district'))

        dt = GeoUtils.get_db(text(query), gis_db=False)

        for row in dt:
            drink_source = row.get('drink_source').split(';')
            drink_source_pct = row.get('drink_source_pct').split(';')
            safe_source = row.get('safe_source').split(';')
            safe_source_pct = row.get('safe_source_pct').split(';')
            waste_source = row.get('waste_source').split(';')
            waste_source_pct = row.get('waste_source_pct').split(';')

            social_water['drink_source'] = [{ 'code': source.lower().replace(' ', '_'), 'source': source, 'percentage': drink_source_pct[i] } for i, source in enumerate(drink_source)]
            social_water['safe_source'] = [{ 'source': source, 'percentage': safe_source_pct[i] } for i, source in enumerate(safe_source)]
            social_water['waste_source'] = [{ 'source': source, 'percentage': waste_source_pct[i] } for i, source in enumerate(waste_source)]

        return social_water