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
        }
    
    def get_people_cs_demography(self) -> dict:
        data = GeoUtils.get_db_function_data(
            func='v2_tha_demography',
            col=[
                'latest_year','population_total','population_male','population_female','population_houses',
                'population_density_per_sq_km','migrants_in','migrants_out','pyramid_male_value','pyramid_male_age_group','pyramid_female_value',
                'pyramid_female_age_group','population_growth_pct'
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
            func='v2_tha_employment',
            col=[
                'latest_year','quarter','unemployed_male_total','unemployed_male_total_pct','unemployed_female_total',
                'unemployed_female_total_pct','employed_sector_category','employed_sector_value','employed_occupation_category',
                'employed_occupation_value','employed_economic_activity_category','employed_economic_activity_value',
                'employment_trend_quarter','employment_trend_year','unemployment_trend_pct','employment_trend_pct'
            ],
            params=self.location.get('province')
        )

        data['employment_trend_quarter'] = [n for n in GeoUtils.string_to_list(data.get('employment_trend_quarter'), ';') if n]
        data['employment_trend_year'] = [int(n) for n in GeoUtils.string_to_list(data.get('employment_trend_year'), ';') if n]
        data['employment_trend_pct'] = [float(n) for n in GeoUtils.string_to_list(data.get('employment_trend_pct'), ';') if n]
        data['unemployment_trend_pct'] = [float(n) for n in GeoUtils.string_to_list(data.get('unemployment_trend_pct'), ';') if n]

        data['employment_trend_chart'] = []
        for i, quarter in enumerate(data['employment_trend_quarter']):
            d = {
                'year': data['employment_trend_year'][i],
                'quarter': quarter,
                'employment': data['employment_trend_pct'][i],
                'unemployment': data['unemployment_trend_pct'][i],
            }
            data['employment_trend_chart'].append(d)
        
        del data['employment_trend_quarter']
        del data['employment_trend_year']
        del data['employment_trend_pct']
        del data['unemployment_trend_pct']

        return data
    
    def get_people_cs_education(self) -> dict:
        data = GeoUtils.get_db_function_data(
            func='v2_tha_education',
            col=[
                'latest_year','attainment_top_3_value','attainment_top_3_category','attainment_highest_category',
                'attainment_higher_level_male','attainment_higher_level_female','g12_work_latest_period','g12_work_category',
                'g12_work_value','literacy_rate_male_pct','literacy_rate_female_pct','literacy_rate_aged_6_total',
                'literacy_rate_aged_6_male','literacy_rate_aged_6_female','literacy_rate_can_read_not_study_male',
                'literacy_rate_can_read_not_study_female','education_infrastructure_school','education_infrastructure_student','education_infrastructure_classroom'
            ],
            params=self.location.get('province')
        )

        data['attainment_top_3_category'] = [n for n in GeoUtils.string_to_list(data.get('attainment_top_3_category'), ';') if n]
        data['attainment_top_3_value'] = [float(n) for n in GeoUtils.string_to_list(data.get('attainment_top_3_value'), ';') if n]
        data['g12_work_latest_period'] = [int(n) for n in GeoUtils.string_to_list(data.get('g12_work_latest_period'), ';') if n]
        data['g12_work_category'] = [n for n in GeoUtils.string_to_list(data.get('g12_work_category'), ';') if n]
        data['g12_work_value'] = [int(n) for n in GeoUtils.string_to_list(data.get('g12_work_value'), ';') if n]

        data['g12_work'] = [{
            'category': n,
            'period': data['g12_work_latest_period'][i],
            'value': data['g12_work_value'][i]
        } for i, n in enumerate(data['g12_work_category'])]
        
        total_attainment_top_3 = sum(data['attainment_top_3_value'])
        data['attainment_top_3'] = [{
            'category': n,
            'value': data['attainment_top_3_value'][i],
            'pct': round(data['attainment_top_3_value'][i]/total_attainment_top_3*100, 2)
        } for i, n in enumerate(data['attainment_top_3_category'])]
        
        del data['attainment_top_3_category']
        del data['attainment_top_3_value']
        del data['g12_work_latest_period']
        del data['g12_work_category']
        del data['g12_work_value']

        return data
    
    def get_people_cs_economy(self) -> dict:
        data = GeoUtils.get_db_function_data(
            func='v2_tha_economy',
            col=[
                'latest_year','gpp_per_capita','gpp_per_capita_year','gpp_sector_category','gpp_sector_value',
                'annual_income_latest_year','annual_income_value','income_monthly',
                'household_expenditure_average','household_expenditure_poverty','household_expenditure_average_pct',
                'household_expenditure_poverty_pct','cpi_latest_year','cpi_value','cpi_group_category','cpi_group_value',
                'land_use_category','land_use_value','land_use_total','crop_rice_planted_area','crop_rice_harvested_area',
                'crop_rice_production','factories_operation_latest_year','factories_operation_total',
                'factories_operation_investment','factories_operation_employee',
                'avg_monthly_income_chart','avg_household_expenditure_chart','poverty_expenditure_chart',
                'avg_monthly_income_year','avg_household_expenditure_year','poverty_expenditure_year',
                'commodity_category', 'commodity_value'
            ],
            params=self.location.get('province')
        )

        data['gpp_sector_category'] = [n for n in GeoUtils.string_to_list(data.get('gpp_sector_category'), '|') if n]
        data['gpp_sector_value'] = [float(n) for n in GeoUtils.string_to_list(data.get('gpp_sector_value'), ';') if n]
        data['cpi_group_category'] = [n for n in GeoUtils.string_to_list(data.get('cpi_group_category'), ';') if n]
        data['cpi_group_value'] = [float(n) for n in GeoUtils.string_to_list(data.get('cpi_group_value'), ';') if n]
        data['avg_monthly_income_chart'] = [float(n) for n in GeoUtils.string_to_list(data.get('avg_monthly_income_chart'), ';') if n]
        data['avg_monthly_income_year'] = [int(n) for n in GeoUtils.string_to_list(data.get('avg_monthly_income_year'), ';') if n]
        data['avg_household_expenditure_chart'] = [float(n) for n in GeoUtils.string_to_list(data.get('avg_household_expenditure_chart'), ';') if n]
        data['avg_household_expenditure_year'] = [int(n) for n in GeoUtils.string_to_list(data.get('avg_household_expenditure_year'), ';') if n]
        data['poverty_expenditure_chart'] = [float(n) for n in GeoUtils.string_to_list(data.get('poverty_expenditure_chart'), ';') if n]
        data['poverty_expenditure_year'] = [int(n) for n in GeoUtils.string_to_list(data.get('poverty_expenditure_year'), ';') if n]
        data['commodity_category'] = [n for n in GeoUtils.string_to_list(data.get('commodity_category'), '|') if n]
        data['commodity_value'] = [float(n) for n in GeoUtils.string_to_list(data.get('commodity_value'), ';') if n]

        data['gpp_sector'] = [{
            'category': n,
            'value': data['gpp_sector_value'][i]
        } for i, n in enumerate(data['gpp_sector_category'])]
        data['cpi_group'] = [{
            'category': n,
            'value': data['cpi_group_value'][i]
        } for i, n in enumerate(data['cpi_group_category'])]

        data['avg_monthly_income'] = {n: data['avg_monthly_income_chart'][i] for i, n in enumerate(data['avg_monthly_income_year'])}
        
        data['expenditure_chart'] = []
        for i, year in enumerate(data['avg_household_expenditure_year']):
            d = {
                'year': year,
                'avg_household_expenditure': data['avg_household_expenditure_chart'][i],
                'poverty_expenditure': data['poverty_expenditure_chart'][i],
                'avg_monthly_income': data['avg_monthly_income'].get(year)
            }
            data['expenditure_chart'].append(d)
        
        data['commodity'] = [{
            'category': n,
            'value': data['commodity_value'][i]
        } for i, n in enumerate(data['commodity_category'])]
        
        del data['gpp_sector_category']
        del data['gpp_sector_value']
        del data['cpi_group_category']
        del data['cpi_group_value']
        del data['avg_monthly_income_chart']
        del data['avg_monthly_income_year']
        del data['avg_household_expenditure_chart']
        del data['avg_household_expenditure_year']
        del data['poverty_expenditure_chart']
        del data['poverty_expenditure_year']
        del data['avg_monthly_income']
        del data['commodity_category']
        del data['commodity_value']

        return data
    
    def get_people_cs_health(self) -> dict:
        data = GeoUtils.get_db_function_data(
            func='v2_tha_health',
            col=[
                'latest_year','medical_workers_category','medical_workers_value','disabled_latest_year','disabled_category',
                'disabled_sex','disabled_value','disease_latest_year','disease_category','disease_value',
                'hospital_latest_year', 'hospital_category', 'hospital_value'
            ],
            params=self.location.get('province')
        )

        data['medical_workers_category'] = [n for n in GeoUtils.string_to_list(data.get('medical_workers_category'), ';') if n]
        data['medical_workers_value'] = [int(n) for n in GeoUtils.string_to_list(data.get('medical_workers_value'), ';') if n]
        data['disabled_category'] = [n for n in GeoUtils.string_to_list(data.get('disabled_category'), ';') if n]
        data['disabled_sex'] = [n for n in GeoUtils.string_to_list(data.get('disabled_sex'), ';') if n]
        data['disabled_value'] = [n for n in GeoUtils.string_to_list(data.get('disabled_value'), ';') if n]
        data['disease_category'] = [n for n in GeoUtils.string_to_list(data.get('disease_category'), ';') if n]
        data['disease_value'] = [int(n) for n in GeoUtils.string_to_list(data.get('disease_value'), ';') if n]

        data['medical_workers'] = [{
            'category': n,
            'value': data['medical_workers_value'][i]
        } for i, n in enumerate(data['medical_workers_category'])]
        data['disease'] = [{
            'category': n,
            'value': data['disease_value'][i]
        } for i, n in enumerate(data['disease_category'])]

        data['disability_by_type_and_gender'] = []
        for i, category in enumerate(data['disabled_category']):
            sex = data['disabled_sex'][i].split('|')
            value = data['disabled_value'][i].split('|')
            d = {
                'category': category,
            }
            for j, s in enumerate(sex):
                d[s] = int(value[j])
            data['disability_by_type_and_gender'].append(d)
        
        del data['medical_workers_category']
        del data['medical_workers_value']
        del data['disabled_category']
        del data['disabled_sex']
        del data['disabled_value']
        del data['disease_category']
        del data['disease_value']

        data['malnutrition_data'] = self.get_people_cs_health_malnutrition()

        data['hospital_category'] = [n for n in GeoUtils.string_to_list(data.get('hospital_category'), ';') if n]
        data['hospital_value'] = [int(n) for n in GeoUtils.string_to_list(data.get('hospital_value'), ';') if n]
        
        data['hospital'] = [{
            'category': n,
            'value': data['hospital_value'][i]
        } for i, n in enumerate(data['hospital_category'])]

        data['hospital_total'] = 0
        for hospital in data['hospital']:
            data['hospital_total'] += hospital.get('value')
        
        del data['hospital_category']
        del data['hospital_value']

        return data
    
    def get_people_cs_health_malnutrition(self) -> dict:
        data = GeoUtils.get_db_function_data(
            func='v2_tha_health_malnutrition',
            col=[
                'latest_year','malnutrition_category','malnutrition_age_group','malnutrition_value'
            ],
            params=self.location.get('district')
        )

        data['malnutrition_category'] = [n for n in GeoUtils.string_to_list(data.get('malnutrition_category'), ';') if n]
        data['malnutrition_age_group'] = [n for n in GeoUtils.string_to_list(data.get('malnutrition_age_group'), ';') if n]
        data['malnutrition_value'] = [float(n) for n in GeoUtils.string_to_list(data.get('malnutrition_value'), ';') if n]
        data['malnutrition_chart'] = {}
        for i, category in enumerate(data['malnutrition_category']):
            age_group = data['malnutrition_age_group'][i]
            if not data['malnutrition_chart'].get(age_group):
                data['malnutrition_chart'][age_group] = {
                    'obesity': 0,
                    'stunting': 0,
                    'thinness': 0
                }
            data['malnutrition_chart'][age_group][category.lower()] += data['malnutrition_value'][i]
        del data['malnutrition_category']
        del data['malnutrition_age_group']
        del data['malnutrition_value']

        return data
    
    def get_people_cs_hhs(self) -> dict:
        return GeoUtils.get_db_function_data(
            func='v2_tha_housing_human_settlement',
            col=[
                'district','latest_year','balance_of_water','groundwater_quality','groundwater_used_vs_usable','water_consumption_per_capita',
                'groudwater_availability_per_capita','annual_stored_water_per_capita','stored_water_per_agricultural_area',
                'rural_households_access_water','urban_households_access_water','govrnmeent_offices_access_water',
                'households_good_quality_water','manufacturing_water_quality','services_water_quality',
                'flood_prone_area_vs_total_area','urban_area_flood_zone_vs_total_urban_area','landslide_prone_vs_total_area',
                'village_with_wastewater_treatment','avg_annual_runoff_per_capita','waste_solid_total','waste_solid_in_municipality',
                'waste_solid_outside_municipality'
            ],
            params=self.location.get('district')
        )