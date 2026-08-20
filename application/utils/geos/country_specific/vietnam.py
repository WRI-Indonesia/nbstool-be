from sqlalchemy import text
from .. import GeoUtils

class Social():
    
    def __init__(self, location:dict):
        self.location = location
    
    def get_social_country_specific(self) -> dict:
        return {
            'demography': self.get_people_cs_demography_social(),
            'employment': self.get_people_cs_employment(),
            'education': self.get_people_cs_education(),
            'economy': self.get_people_cs_economy(),
            'health': self.get_people_cs_health(),
            'hhs': self.get_people_cs_hhs(),
            'land_use': self.get_people_cs_land_use(),
        }
    
    def get_people_cs_demography_social(self) -> dict:
        return GeoUtils.get_db_function_data(
            func='v2_vnm_demography_social',
            col=[
                'latest_year', 'population_total', 'population_male', 'population_male_pct', 'population_female', 'population_female_pct',
                'population_density', 'population_growth_pct', 'immigration_rate_pct', 'immigration_value',
                'emigration_rate_pct', 'emigration_value'
            ],
            params=self.location.get('province')
        )
    
    def get_people_cs_employment(self) -> dict:
        employment = GeoUtils.get_db_function_data(
            func='v2_vnm_employment',
            col=[
                'latest_year', 'labour_force', 'employment_value', 'employment_rate_pct', 'employment_rate_prev_year_pct',
                'employment_rate_diff_pct', 'unemployment_rate_pct', 'unemployment_rate_prev_year_pct', 'unemployment_rate_diff_pct',
                'underemployment_rate_pct', 'underemployment_rate_prev_year_pct', 'underemployment_rate_diff_pct', 'trend_year',
                'trend_employment', 'trend_unemployment', 'trend_underemployment'
            ],
            params=self.location.get('province')
        )

        employment['trend_year'] = employment.get('trend_year').split(';') if employment.get('trend_year') else []
        employment['trend_employment'] = employment.get('trend_employment').split(';') if employment.get('trend_employment') else []
        employment['trend_unemployment'] = employment.get('trend_unemployment').split(';') if employment.get('trend_unemployment') else []
        employment['trend_underemployment'] = employment.get('trend_underemployment').split(';') if employment.get('trend_underemployment') else []
        employment['trend_chart'] = []
        for i, year in enumerate(employment.get('trend_year')):
            d = {
                'year': int(year),
                'employment': float(employment.get('trend_employment')[i]),
                'unemployment': float(employment.get('trend_unemployment')[i]),
                'underemployment': float(employment.get('trend_underemployment')[i]),
            }
            employment['trend_chart'].append(d)
        del employment['trend_year']
        del employment['trend_employment']
        del employment['trend_unemployment']
        del employment['trend_underemployment']

        return employment
    
    def get_people_cs_education(self) -> dict:
        return GeoUtils.get_db_function_data(
            func='v2_vnm_education',
            col=[
                'latest_year','education_literate_pct','education_literate_male_pct','education_literate_female_pct',
                'teacher_kindergarten','teacher_general','teacher_vocational','teacher_universities','student_kindergarten',
                'student_general','student_vocational','student_universities','teacher_general_minority','teacher_general_woman',
                'student_general_minority','student_general_schoolgirl'
            ],
            params=self.location.get('province')
        )
    
    def get_people_cs_economy(self) -> dict:
        economy = GeoUtils.get_db_function_data(
            func='v2_vnm_economy',
            col=[
                'latest_year','top_3_commodities_category','top_3_commodities_value','top_3_commodities_unit','gdp_industries_category',
                'gdp_industries_value','gdp_per_capita','monthly_income_per_capita','gini_ratio_pct','gini_ratio_year','cpi_value',
                'cpi_year','cpi_month','social_insurance_pct','health_insurance_pct','unemployment_insurance_pct', 'gdp_growth'
            ],
            params=self.location.get('province')
        )

        economy['top_3_commodities_category'] = GeoUtils.string_to_list(economy.get('top_3_commodities_category'), ';')
        economy['top_3_commodities_unit'] = GeoUtils.string_to_list(economy.get('top_3_commodities_unit'), ';')
        economy['top_3_commodities_value'] = GeoUtils.string_to_list(economy.get('top_3_commodities_value'), ';')
        economy['top_3_commodities'] = [{
            'category': n,
            'code': n.lower().replace(' ', '_'),
            'unit': economy.get('top_3_commodities_unit')[i],
            'value': float(economy.get('top_3_commodities_value')[i])
        } for i, n in enumerate(economy.get('top_3_commodities_category'))]
        del economy['top_3_commodities_category']
        del economy['top_3_commodities_unit']
        del economy['top_3_commodities_value']

        economy['gdp_industries_category'] = GeoUtils.string_to_list(economy.get('gdp_industries_category'), ';')
        economy['gdp_industries_value'] = GeoUtils.string_to_list(economy.get('gdp_industries_value'), ';')
        economy['gdp_industries'] = [{
            'category': n,
            'value': float(economy.get('gdp_industries_value')[i])
        } for i, n in enumerate(economy.get('gdp_industries_category'))]
        del economy['gdp_industries_category']
        del economy['gdp_industries_value']

        return economy
    
    def get_people_cs_health(self) -> dict:
        health = GeoUtils.get_db_function_data(
            func='v2_vnm_health',
            col=[
                'latest_year','health_facilities_category','health_facilities_value','health_facilities_total',
                'medical_staff_category','medical_staff_value','aids_deaths_latest_year','aids_deaths',
                'height_for_age_malnutrition','children_vaccination_latest_year','children_vaccination_year',
                'children_vaccination_value'
            ],
            params=self.location.get('province')
        )
        
        health['health_facilities_category'] = GeoUtils.string_to_list(health.get('health_facilities_category'), ';')
        health['health_facilities_value'] = GeoUtils.string_to_list(health.get('health_facilities_value'), ';')
        health['health_facilities'] = [{
            'category': n,
            'value': int(health.get('health_facilities_value')[i])
        } for i, n in enumerate(health.get('health_facilities_category'))]
        del health['health_facilities_category']
        del health['health_facilities_value']

        health['medical_staff_category'] = GeoUtils.string_to_list(health.get('medical_staff_category'), ';')
        health['medical_staff_value'] = GeoUtils.string_to_list(health.get('medical_staff_value'), ';')
        health['medical_staff'] = [{
            'category': n,
            'value': int(health.get('medical_staff_value')[i])
        } for i, n in enumerate(health.get('medical_staff_category'))]
        del health['medical_staff_category']
        del health['medical_staff_value']

        health['aids_deaths'] = [int(n) for n in GeoUtils.string_to_list(health.get('aids_deaths'), ';')]
        health['aids_deaths_5_years_total'] = sum(health.get('aids_deaths'))
        health['aids_deaths_5_years_avg'] = int(health['aids_deaths_5_years_total'] / len(health.get('aids_deaths')))
        del health['aids_deaths']

        health['children_vaccination_year'] = GeoUtils.string_to_list(health.get('children_vaccination_year'), ';')
        health['children_vaccination_value'] = GeoUtils.string_to_list(health.get('children_vaccination_value'), ';')
        health['children_vaccination_latest_year'] = int(health['children_vaccination_year'][-1])
        health['children_vaccination_latest_value'] = float(health['children_vaccination_value'][-1])
        health['children_vaccination_diff'] = float(health['children_vaccination_value'][-1]) - float(health['children_vaccination_value'][-2])
        if health['children_vaccination_diff'] > 0:
            health['children_vaccination_status'] = 'Increase'
        else:
            health['children_vaccination_status'] = 'Decrease'
        del health['children_vaccination_year']
        del health['children_vaccination_value']

        return health
    
    def get_people_cs_hhs(self) -> dict:
        hhs = GeoUtils.get_db_function_data(
            func='v2_vnm_housing_human_settlement',
            col=[
                'latest_year','national_electricity_latest_year','national_electricity_pct','house_structures_category',
                'house_structures_value','water_source_latest_year','water_source_hygienic_pct','water_source_centralized_pct',
                'solid_waste_latest_year','solid_waste_collected','solid_waste_collected_national_criteria','toilet_usage_pct'
            ],
            params=self.location.get('province')
        )

        hhs['house_structures_category'] = GeoUtils.string_to_list(hhs.get('house_structures_category'), ';')
        hhs['house_structures_value'] = GeoUtils.string_to_list(hhs.get('house_structures_value'), ';')
        hhs['house_structures'] = [{
            'category': n,
            'value': int(hhs.get('house_structures_value')[i])
        } for i, n in enumerate(hhs.get('house_structures_category'))]
        del hhs['house_structures_category']
        del hhs['house_structures_value']

        return hhs
    
    def get_people_cs_land_use(self) -> dict:
        return GeoUtils.get_db_function_data(
            func='v2_vnm_land_use',
            col=[
                'latest_year','agricultural_production_land','forestry_land','specially_used_land','homestead_land'
            ],
            params=self.location.get('province')
        )