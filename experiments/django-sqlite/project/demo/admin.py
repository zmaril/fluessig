"""Register the generated demo models with the admin site.

Done generically over the app's model registry so a schema change (new table →
new model) is picked up without editing this file.

KNOWN DJANGO FRICTION: as of Django 5.2, a model with a ``CompositePrimaryKey``
CANNOT be registered with the admin — ``ModelAdmin`` raises
``ImproperlyConfigured("... has a composite primary key, so it cannot be
registered with admin.")``. So our composite-PK models (Issues, Labels,
IssueLabels) are admin-invisible under the default admin. We register the
single-PK models and record the skipped ones on ``UNREGISTERABLE_COMPOSITE_PK``
so the experiment reports the gap honestly rather than swallowing it.
"""
from django.apps import apps
from django.contrib import admin
from django.core.exceptions import ImproperlyConfigured

UNREGISTERABLE_COMPOSITE_PK = []

for model in apps.get_app_config("demo").get_models():
    try:
        admin.site.register(model)
    except ImproperlyConfigured as exc:
        UNREGISTERABLE_COMPOSITE_PK.append((model.__name__, str(exc)))
