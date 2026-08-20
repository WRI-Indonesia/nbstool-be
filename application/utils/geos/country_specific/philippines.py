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
            'land_distribution': self.get_people_cs_land_distribution(),
        }
    
    def get_people_cs_demography(self) -> dict:
        data = GeoUtils.get_db_function_data(
            func='v2_phl_demography',
            col=[
                'latest_year','region','population_total','population_male','population_female','households_total',
                'population_density','population_growth_pct','pyramid_male_value','pyramid_male_age_group','pyramid_female_value',
                'pyramid_female_age_group','inter_regional_in_migrants','inter_regional_out_migrants','intra_regional_migrants',
                'net_number_of_migrants'
            ],
            params=self.location.get('province')
        )

        data['pyramid_male_value'] = [int(n) for n in GeoUtils.string_to_list(data.get('pyramid_male_value'), ';') if n]
        data['pyramid_male_age_group'] = [n for n in GeoUtils.string_to_list(data.get('pyramid_male_age_group'), ';') if n]
        data['pyramid_female_value'] = [int(n) for n in GeoUtils.string_to_list(data.get('pyramid_female_value'), ';') if n]
        data['pyramid_female_age_group'] = [n for n in GeoUtils.string_to_list(data.get('pyramid_female_age_group'), ';') if n]
        data['pyramid_male'] = {n: data['pyramid_male_value'][i] for i, n in enumerate(data['pyramid_male_age_group'])}
        data['pyramid_female'] = {n: data['pyramid_female_value'][i] for i, n in enumerate(data['pyramid_female_age_group'])}
        data['population_pyramid'] = []
        for i, age_group in enumerate(data['pyramid_male_age_group']):
            d = {
                'male': data['pyramid_male'].get(age_group),
                'female': data['pyramid_female'].get(age_group),
                'age_group': age_group
            }
            data['population_pyramid'].append(d)
        del data['pyramid_male_value']
        del data['pyramid_male_age_group']
        del data['pyramid_female_value']
        del data['pyramid_female_age_group']
        del data['pyramid_male']
        del data['pyramid_female']

        return data
    
    def get_people_cs_employment(self) -> dict:
        data = GeoUtils.get_db_function_data(
            func='v2_phl_employment',
            col=[
                'latest_year','region','underemployment_rate','unemployment_rate','employment_rate','labor_force_participation_rate',
                'employment_top_10_category','underemployment_rate_chart','unemployment_rate_chart','employment_rate_chart',
                'labor_force_participation_rate_chart','underemployment_year_chart','unemployment_year_chart','employment_year_chart',
                'labor_force_participation_year_chart','most_prominent_sector_year','most_prominent_sector','primarily_working_as'
            ],
            params=self.location.get('province')
        )

        data['employment_top_10_category'] = [n for n in GeoUtils.string_to_list(data.get('employment_top_10_category'), ';') if n]
        data['underemployment_rate_chart'] = [float(n) for n in GeoUtils.string_to_list(data.get('underemployment_rate_chart'), ';') if n]
        data['unemployment_rate_chart'] = [float(n) for n in GeoUtils.string_to_list(data.get('unemployment_rate_chart'), ';') if n]
        data['employment_rate_chart'] = [float(n) for n in GeoUtils.string_to_list(data.get('employment_rate_chart'), ';') if n]
        data['labor_force_participation_rate_chart'] = [float(n) for n in GeoUtils.string_to_list(data.get('labor_force_participation_rate_chart'), ';') if n]
        data['underemployment_year_chart'] = [int(n) for n in GeoUtils.string_to_list(data.get('underemployment_year_chart'), ';') if n]
        data['unemployment_year_chart'] = [int(n) for n in GeoUtils.string_to_list(data.get('unemployment_year_chart'), ';') if n]
        data['employment_year_chart'] = [int(n) for n in GeoUtils.string_to_list(data.get('employment_year_chart'), ';') if n]
        data['labor_force_participation_year_chart'] = [int(n) for n in GeoUtils.string_to_list(data.get('labor_force_participation_year_chart'), ';') if n]

        data['employment_status_trend_chart'] = []
        for i, year in enumerate(data['underemployment_year_chart']):
            d = {
                'year': year,
                'employment_rate': data['employment_rate_chart'][i],
                'labor_force_participation_rate': data['labor_force_participation_rate_chart'][i],
                'underemployment_rate': data['underemployment_rate_chart'][i],
                'unemployment_rate': data['unemployment_rate_chart'][i],
            }
            data['employment_status_trend_chart'].append(d)
        
        del data['underemployment_rate_chart']
        del data['unemployment_rate_chart']
        del data['employment_rate_chart']
        del data['labor_force_participation_rate_chart']
        del data['underemployment_year_chart']
        del data['unemployment_year_chart']
        del data['employment_year_chart']
        del data['labor_force_participation_year_chart']

        return data
    
    def get_people_cs_education(self) -> dict:
        data = GeoUtils.get_db_function_data(
            func='v2_phl_education',
            col=[
                'latest_year','region','employed_college_graduate','employed_college_undergraduate','graduated_from_school',
                'graduated_from_college','employed_most_common_grade_category','employed_most_common_grade_value',
                'literacy_rate_latest_year','literacy_rate_male','literacy_rate_female','literacy_rate_male_value',
                'literacy_rate_male_year','literacy_rate_female_value','literacy_rate_female_year','school_latest_year',
                'school_public_shs','school_private_shs','school_private_value','school_private_year','school_public_value',
                'school_public_year','school_total_value','school_total_year'
            ],
            params=self.location.get('province')
        )

        data['literacy_rate_male_value'] = [float(n) for n in GeoUtils.string_to_list(data.get('literacy_rate_male_value'), ';') if n]
        data['literacy_rate_male_year'] = [int(n) for n in GeoUtils.string_to_list(data.get('literacy_rate_male_year'), ';') if n]
        data['literacy_rate_female_value'] = [float(n) for n in GeoUtils.string_to_list(data.get('literacy_rate_female_value'), ';') if n]
        data['literacy_rate_female_year'] = [int(n) for n in GeoUtils.string_to_list(data.get('literacy_rate_female_year'), ';') if n]
        data['school_private_value'] = [int(n) for n in GeoUtils.string_to_list(data.get('school_private_value'), ';') if n]
        data['school_private_year'] = [n for n in GeoUtils.string_to_list(data.get('school_private_year'), ';') if n]
        data['school_public_value'] = [int(n) for n in GeoUtils.string_to_list(data.get('school_public_value'), ';') if n]
        data['school_public_year'] = [n for n in GeoUtils.string_to_list(data.get('school_public_year'), ';') if n]
        data['school_total_value'] = [int(n) for n in GeoUtils.string_to_list(data.get('school_total_value'), ';') if n]
        data['school_total_year'] = [n for n in GeoUtils.string_to_list(data.get('school_total_year'), ';') if n]

        data['literacy_rate'] = [{
            'year': n,
            'male': data['literacy_rate_male_value'][i],
            'female': data['literacy_rate_female_value'][i]
        } for i, n in enumerate(data['literacy_rate_male_year'])]

        data['school_period'] = {}
        for i, period in enumerate(data['school_private_year']):
            if not data['school_period'].get(period):
                data['school_period'][period] = {
                    'public': 0,
                    'private': 0,
                    'total': 0
                }
            
            data['school_period'][period]['private'] += data['school_private_value'][i]
        
        for i, period in enumerate(data['school_public_year']):
            if not data['school_period'].get(period):
                data['school_period'][period] = {
                    'public': 0,
                    'private': 0,
                    'total': 0
                }
            
            data['school_period'][period]['public'] += data['school_public_value'][i]
        
        for i, period in enumerate(data['school_total_year']):
            if not data['school_period'].get(period):
                data['school_period'][period] = {
                    'public': 0,
                    'private': 0,
                    'total': 0
                }
            
            data['school_period'][period]['total'] += data['school_total_value'][i]
        
        data['education_infrastructure'] = [{
            'period': n,
            'public': data['school_period'][n].get('public'),
            'private': data['school_period'][n].get('private'),
            'total': data['school_period'][n].get('total'),
        } for n in data['school_period'].keys()]

        del data['literacy_rate_male_value']
        del data['literacy_rate_male_year']
        del data['literacy_rate_female_value']
        del data['literacy_rate_female_year']
        del data['school_private_value']
        del data['school_private_year']
        del data['school_public_value']
        del data['school_public_year']
        del data['school_total_value']
        del data['school_total_year']
        del data['school_period']

        return data

    def get_people_cs_economy(self) -> dict:
        data = GeoUtils.get_db_function_data(
            func='v2_phl_economy',
            col=[
                'latest_year','region','gdp_value','gdp_growth', 'gdp_total', 'gdp_contribute_pct','gdp_value_per_year','gdp_year_per_year','gdp_growth_per_year',
                'gini_ratio_pct','gdp_current_price','gdp_current_price_per_capita','average_income','average_expenditure','cpi_pct',
                'poverty_incidence_families_pct','poverty_per_capita_php','poverty_severity_pct','gini_ratio_line','gini_ratio_line_year',
                'poverty_line','poverty_line_year', 'gdp_sector_category'
            ],
            params=self.location.get('province')
        )

        data['gdp_value_per_year'] = [int(n) for n in GeoUtils.string_to_list(data.get('gdp_value_per_year'), ';') if n]
        data['gdp_year_per_year'] = [int(n) for n in GeoUtils.string_to_list(data.get('gdp_year_per_year'), ';') if n]
        data['gdp_growth_per_year'] = [float(n) for n in GeoUtils.string_to_list(data.get('gdp_growth_per_year'), ';') if n]
        data['gini_ratio_line'] = [float(n) for n in GeoUtils.string_to_list(data.get('gini_ratio_line'), ';') if n]
        data['gini_ratio_line_year'] = [int(n) for n in GeoUtils.string_to_list(data.get('gini_ratio_line_year'), ';') if n]
        data['poverty_line'] = [float(n) for n in GeoUtils.string_to_list(data.get('poverty_line'), ';') if n]
        data['poverty_line_year'] = [int(n) for n in GeoUtils.string_to_list(data.get('poverty_line_year'), ';') if n]
        data['gdp_sector_category'] = [n.split('_')[0] for n in GeoUtils.string_to_list(data.get('gdp_sector_category'), ';') if n]

        data['gini_ratio'] = {n:data['gini_ratio_line'][i] for i, n in enumerate(data['gini_ratio_line_year'])}
        data['poverty'] = {n:data['poverty_line'][i] for i, n in enumerate(data['poverty_line_year'])}

        data['gdp_chart'] = []
        for i, year in enumerate(data['gdp_year_per_year']):
            d = {
                'year': year,
                'value': data['gdp_value_per_year'][i],
                'growth': data['gdp_growth_per_year'][i],
                'gini_ratio': data['gini_ratio'].get(year),
                'poverty': data['poverty'].get(year)
            }
            data['gdp_chart'].append(d)
        
        del data['gdp_value_per_year']
        del data['gdp_year_per_year']
        del data['gdp_growth_per_year']
        del data['gini_ratio_line']
        del data['gini_ratio_line_year']
        del data['poverty_line']
        del data['poverty_line_year']
        del data['gini_ratio']
        del data['poverty']

        return data
    
    def get_people_cs_health(self) -> dict:
        data = GeoUtils.get_db_function_data(
            func='v2_phl_health',
            col=[
                'latest_year','region','hospital_total','hospital_private','hospital_government',
                'medical_doctors','medical_dentists','medical_nurses','medical_midwives',
                'prevalance_malnutrition_age_group','prevalance_malnutrition_category','prevalance_malnutrition_value'
            ],
            params=self.location.get('province')
        )

        data['prevalance_malnutrition_age_group'] = [n.replace('Less than 5 yo', '0-5 yo').replace(' yo', '') for n in GeoUtils.string_to_list(data.get('prevalance_malnutrition_age_group'), ';') if n]
        data['prevalance_malnutrition_category'] = [n for n in GeoUtils.string_to_list(data.get('prevalance_malnutrition_category'), ';') if n]
        data['prevalance_malnutrition_value'] = [float(n) for n in GeoUtils.string_to_list(data.get('prevalance_malnutrition_value'), ';') if n]

        data['prevalance_malnutrition'] = {}
        for i, age_group in enumerate(data['prevalance_malnutrition_age_group']):
            if not data['prevalance_malnutrition'].get(age_group):
                data['prevalance_malnutrition'][age_group] = { }
            data['prevalance_malnutrition'][age_group][data['prevalance_malnutrition_category'][i].split('_')[0]] = data['prevalance_malnutrition_value'][i]
        
        del data['prevalance_malnutrition_age_group']
        del data['prevalance_malnutrition_category']
        del data['prevalance_malnutrition_value']

        return data
    
    def get_people_cs_hhs(self) -> dict:
        data = GeoUtils.get_db_function_data(
            func='v2_phl_housing_human_settlement',
            col=[
                'latest_year','region','drinking_water_supply_category','drinking_water_supply_value',
                'wastewater_category','wastewater_value'
            ],
            params=self.location.get('province')
        )

        data['drinking_water_supply_category'] = [n for n in GeoUtils.string_to_list(data.get('drinking_water_supply_category'), ';') if n]
        data['drinking_water_supply_value'] = [int(n) for n in GeoUtils.string_to_list(data.get('drinking_water_supply_value'), ';') if n]
        data['wastewater_category'] = [n for n in GeoUtils.string_to_list(data.get('wastewater_category'), ';') if n]
        data['wastewater_value'] = [int(n) for n in GeoUtils.string_to_list(data.get('wastewater_value'), ';') if n]

        data['drinking_water_supply'] = [{
            'category': n,
            'code': n.lower().replace(' ', '_').replace('/', '_'),
            'value': data['drinking_water_supply_value'][i]
        } for i, n in enumerate(data['drinking_water_supply_category'])]
        data['wastewater'] = [{
            'category': n,
            'value': data['wastewater_value'][i]
        } for i, n in enumerate(data['wastewater_category'])]

        del data['drinking_water_supply_category']
        del data['drinking_water_supply_value']
        del data['wastewater_category']
        del data['wastewater_value']

        return data
    
    def get_agriculture_land_distribution_category(self, s):
        s = s.split('_')[0]
        category = {
            'GOL/KKK': 'Government-owned (GOL)',
            'LES': 'Land Estates (LE)',
            'SETT': 'Settlements (SETT)',

            'CA': 'Compulsory Acquisition (CA)',
            'GFI': 'Lands Foreclosed by Government Financial Institutions (GFI)',
            'OLT': 'Operation Land Transfer',
            'VLT': 'Voluntary Land Transfer/Direct Payment Scheme (VLT/DPS)',
            'VOS': 'Voluntary Offer to Sell (VOS)',
        }
        return category.get(s)
    
    def get_people_cs_land_distribution(self) -> dict:
        data = GeoUtils.get_db_function_data(
            func='v2_phl_land_distribution',
            col=[
                'latest_year','region','land_distribution_private_agriculture','land_distribution_non_private_agriculture',
                'agriculture_distribution_private_category', 'agriculture_distribution_private_value', 'agriculture_distribution_non_private_category', 'agriculture_distribution_non_private_value'
            ],
            params=self.location.get('province')
        )

        data['agriculture_distribution_private_category'] = [self.get_agriculture_land_distribution_category(n) for n in GeoUtils.string_to_list(data.get('agriculture_distribution_private_category'), ';') if n]
        data['agriculture_distribution_private_value'] = [float(n) for n in GeoUtils.string_to_list(data.get('agriculture_distribution_private_value'), ';') if n]
        data['agriculture_distribution_non_private_category'] = [self.get_agriculture_land_distribution_category(n) for n in GeoUtils.string_to_list(data.get('agriculture_distribution_non_private_category'), ';') if n]
        data['agriculture_distribution_non_private_value'] = [float(n) for n in GeoUtils.string_to_list(data.get('agriculture_distribution_non_private_value'), ';') if n]

        agriculture_distribution_private_total = sum(data['agriculture_distribution_private_value'])
        agriculture_distribution_non_private_total = sum(data['agriculture_distribution_non_private_value'])
        
        data['agriculture_distribution_private'] = [{
            'category': n,
            'value': data['agriculture_distribution_private_value'][i],
            'pct': round(data['agriculture_distribution_private_value'][i]/agriculture_distribution_private_total*100, 2)
        } for i, n in enumerate(data['agriculture_distribution_private_category'])]

        data['agriculture_distribution_non_private'] = [{
            'category': n,
            'value': data['agriculture_distribution_non_private_value'][i],
            'pct': round(data['agriculture_distribution_non_private_value'][i]/agriculture_distribution_non_private_total*100, 2)
        } for i, n in enumerate(data['agriculture_distribution_non_private_category'])]

        del data['agriculture_distribution_private_category']
        del data['agriculture_distribution_private_value']
        del data['agriculture_distribution_non_private_category']
        del data['agriculture_distribution_non_private_value']

        return data