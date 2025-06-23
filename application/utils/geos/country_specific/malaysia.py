from sqlalchemy import text
from .. import GeoUtils

class Social():
    
    def __init__(self, location:dict):
        self.location = location
    
    def get_social_country_specific(self) -> dict:
        return {
            'demography': self.get_people_cs_demography(),
            'employment': self.get_people_cs_employment(),
            'education': self.get_people_cs_education(),
            'economy': self.get_people_cs_economy(),
            'health': self.get_people_cs_health(),
            'hhs': self.get_people_cs_hhs(),
            'land_use': self.get_people_cs_land_use(),
        }
    
    def get_people_cs_demography(self) -> dict:
        data = GeoUtils.get_db_function_data(
            func='v2_mys_demography',
            col=[
                'latest_year','district','population_total','population_growth','population_density','households_latest_period',
                'number_of_households','pyramid_male_value','pyramid_male_age','pyramid_female_value','pyramid_female_age'
            ],
            params=self.location.get('district')
        )

        data['pyramid_male_value'] = [int(n) for n in GeoUtils.string_to_list(data.get('pyramid_male_value'), ';') if n]
        data['pyramid_male_age'] = [n for n in GeoUtils.string_to_list(data.get('pyramid_male_age'), ';') if n]
        data['pyramid_female_value'] = [int(n) for n in GeoUtils.string_to_list(data.get('pyramid_female_value'), ';') if n]
        data['pyramid_female_age'] = [n for n in GeoUtils.string_to_list(data.get('pyramid_female_age'), ';') if n]
        data['pyramid_male'] = {n: data['pyramid_male_value'][i] for i, n in enumerate(data['pyramid_male_age'])}
        data['pyramid_female'] = {n: data['pyramid_female_value'][i] for i, n in enumerate(data['pyramid_female_age'])}
        data['population_pyramid'] = []
        for i, age in enumerate(data['pyramid_male_age']):
            d = {
                'male': data['pyramid_male'].get(age),
                'female': data['pyramid_female'].get(age),
                'age': age
            }
            data['population_pyramid'].append(d)
        del data['pyramid_male_value']
        del data['pyramid_male_age']
        del data['pyramid_female_value']
        del data['pyramid_female_age']
        del data['pyramid_male']
        del data['pyramid_female']

        return data
    
    def get_people_cs_employment(self) -> dict:
        data = GeoUtils.get_db_function_data(
            func='v2_mys_employment',
            col=[
                'latest_year','unemployment_total','employment_total','unemployed_graduated','employed_graduated',
                'unemployed_graduated_rate_data','employed_graduated_rate_data','unemployment_graduated_rate',
                'employment_graduated_rate','employment_industry_category','employment_industry_value',
                'unemployed_graduated_male','unemployed_graduated_female','employed_graduated_male',
                'employed_graduated_female','outside_labour_graduated_male','outside_labour_graduated_female'
            ],
            params=self.location.get('province')
        )

        data['employment_industry_category'] = [n for n in GeoUtils.string_to_list(data.get('employment_industry_category'), '|') if n]
        data['employment_industry_value'] = [int(round(float(n))) for n in GeoUtils.string_to_list(data.get('employment_industry_value'), ';') if n]
        data['employment_industry'] = [{
            'category': n,
            'code': n.lower().replace(' ', '_').replace(';', '').replace(',', '').replace('/', '_'),
            'value': data['employment_industry_value'][i]
        } for i, n in enumerate(data['employment_industry_category'])]
        del data['employment_industry_category']
        del data['employment_industry_value']

        return data
    
    def get_people_cs_education(self) -> dict:
        data = GeoUtils.get_db_function_data(
            func='v2_mys_education',
            col=[
                'latest_year','unemployed_graduated','employed_graduated','outside_labour_graduated',
                'facilities_year','facilities_value','educators_year','educators_value'
            ],
            params=self.location.get('province')
        )

        data['facilities_year'] = [int(n) for n in GeoUtils.string_to_list(data.get('facilities_year'), ';') if n]
        data['facilities_value'] = [int(n) for n in GeoUtils.string_to_list(data.get('facilities_value'), ';') if n]
        data['educators_year'] = [int(n) for n in GeoUtils.string_to_list(data.get('educators_year'), ';') if n]
        data['educators_value'] = [int(n) for n in GeoUtils.string_to_list(data.get('educators_value'), ';') if n]
        data['education_chart_data'] = [{
            'year': n,
            'facilities': data['facilities_value'][i],
            'educators': data['educators_value'][i]
        } for i, n in enumerate(data['facilities_year'])]
        del data['facilities_year']
        del data['facilities_value']
        del data['educators_year']
        del data['educators_value']

        return data
    
    def get_people_cs_economy(self) -> dict:
        data = GeoUtils.get_db_function_data(
            func='v2_mys_economy',
            col=[
                'latest_year','household_income_gross_median','gdp_current_latest_year','gdp_per_capita',
                'gdp_current','gdp_growth','gdp_industry_category','gdp_industry_value','gini_ratio_pct',
                'cpi_index','relative_poverty_incidence','chart_year','chart_gdp','chart_gni','chart_gdp_per_capita'
            ],
            params=self.location.get('province')
        )

        data['gdp_industry_category'] = [n for n in GeoUtils.string_to_list(data.get('gdp_industry_category'), ';') if n]
        data['gdp_industry_value'] = [float(n) for n in GeoUtils.string_to_list(data.get('gdp_industry_value'), ';') if n]
        data['chart_year'] = [int(n) for n in GeoUtils.string_to_list(data.get('chart_year'), ';') if n]
        data['chart_gdp'] = [float(n) if float(n) > 0 else None for n in GeoUtils.string_to_list(data.get('chart_gdp'), ';') if n]
        data['chart_gni'] = [float(n) if float(n) > 0 else None for n in GeoUtils.string_to_list(data.get('chart_gni'), ';') if n]
        data['chart_gdp_per_capita'] = [float(n) if float(n) > 0 else None for n in GeoUtils.string_to_list(data.get('chart_gdp_per_capita'), ';') if n]
        
        data['gdp_industry'] = [{
            'category': n,
            'value': data['gdp_industry_value'][i]
        } for i, n in enumerate(data['gdp_industry_category'])]
        data['economy_chart_data'] = [{
            'year': n,
            'gdp': data['chart_gdp'][i],
            'gni': data['chart_gni'][i],
            'gdp_per_capita': data['chart_gdp_per_capita'][i],
        } for i, n in enumerate(data['chart_year'])]

        del data['gdp_industry_category']
        del data['gdp_industry_value']
        del data['chart_year']
        del data['chart_gdp']
        del data['chart_gni']
        del data['chart_gdp_per_capita']

        return data
    
    def get_people_cs_health(self) -> dict:
        return GeoUtils.get_db_function_data(
            func='v2_mys_health',
            col=[
                'latest_year','public_hospital','private_hospital','child_stunting_year','child_stunting_pct',
                'child_underweight_year','child_underweight_pct', 'obesity_total', 'obesity_pct'
            ],
            params=self.location.get('province')
        )

    def get_people_cs_hhs(self) -> dict:
        data = GeoUtils.get_db_function_data(
            func='v2_mys_housing_human_settlement',
            col=[
                'latest_year','housing_unit_category','housing_unit_value','sanitary_latrines_year','sanitary_latrines_pct',
                'clean_water_supply_year','clean_water_supply_pct','drinking_year','drinking_pct',
                'malaysian_housing_unit_category','malaysian_housing_unit_value','water_chart_year',
                'water_chart_domestic_water_consumption','water_chart_nondomestic_water_consumption','water_chart_water_supplied'
            ],
            params=self.location.get('province')
        )

        data['housing_unit_category'] = [n for n in GeoUtils.string_to_list(data.get('housing_unit_category'), ';') if n]
        data['housing_unit_value'] = [int(n) for n in GeoUtils.string_to_list(data.get('housing_unit_value'), ';') if n]
        data['malaysian_housing_unit_category'] = [n for n in GeoUtils.string_to_list(data.get('malaysian_housing_unit_category'), ';') if n]
        data['malaysian_housing_unit_value'] = [int(n) for n in GeoUtils.string_to_list(data.get('malaysian_housing_unit_value'), ';') if n]
        data['water_chart_year'] = [n for n in GeoUtils.string_to_list(data.get('water_chart_year'), ';') if n]
        data['water_chart_domestic_water_consumption'] = [float(n) for n in GeoUtils.string_to_list(data.get('water_chart_domestic_water_consumption'), ';') if n]
        data['water_chart_nondomestic_water_consumption'] = [float(n) for n in GeoUtils.string_to_list(data.get('water_chart_nondomestic_water_consumption'), ';') if n]
        data['water_chart_water_supplied'] = [float(n) for n in GeoUtils.string_to_list(data.get('water_chart_water_supplied'), ';') if n]
        
        data['housing_unit'] = [{
            'category': n,
            'value': data['housing_unit_value'][i]
        } for i, n in enumerate(data['housing_unit_category'])]
        data['malaysian_housing_unit'] = [{
            'category': n,
            'value': data['malaysian_housing_unit_value'][i]
        } for i, n in enumerate(data['malaysian_housing_unit_category'])]
        data['water_chart'] = [{
            'year': n,
            'domestic_water_consumption': data['water_chart_domestic_water_consumption'][i],
            'nondomestic_water_consumption': data['water_chart_nondomestic_water_consumption'][i],
            'water_supplied': data['water_chart_water_supplied'][i]
        } for i, n in enumerate(data['water_chart_year'])]

        data['water_state_supply_production_perday'] = data['water_chart'][-1].get('water_supplied')
        data['water_state_consumption_domestic_perday'] = data['water_chart'][-1].get('domestic_water_consumption')
        data['water_state_consumption_nondomestic_perday'] = data['water_chart'][-1].get('nondomestic_water_consumption')
        data['water_state_latest_year'] = data['water_chart'][-1].get('year')

        del data['housing_unit_category']
        del data['housing_unit_value']
        del data['malaysian_housing_unit_category']
        del data['malaysian_housing_unit_value']
        del data['water_chart_year']
        del data['water_chart_domestic_water_consumption']
        del data['water_chart_nondomestic_water_consumption']
        del data['water_chart_water_supplied']

        return data
    
    def get_people_cs_land_use(self) -> dict:
        return GeoUtils.get_db_function_data(
            func='v2_mys_land_use',
            col=[
                'latest_year','reserved_forests_area'
            ],
            params=self.location.get('province')
        )