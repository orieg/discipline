#!/usr/bin/env python3
import sys
import json
import subprocess
import shutil

def validate_sarif(sarif_path, schema_path):
    try:
        import jsonschema
        with open(schema_path, "r", encoding="utf-8") as f:
            schema = json.load(f)
        with open(sarif_path, "r", encoding="utf-8") as f:
            instance = json.load(f)
        jsonschema.validate(instance=instance, schema=schema)
        print(f"OK: {sarif_path} validates against {schema_path} via jsonschema")
        return
    except ImportError:
        pass

    # Fallback when jsonschema is not installed in the environment:
    # Perform strict structural validation against SARIF 2.1.0 specification
    with open(sarif_path, "r", encoding="utf-8") as f:
        instance = json.load(f)
    assert isinstance(instance, dict), "SARIF root must be an object"
    assert instance.get("version") == "2.1.0", f"Expected SARIF version 2.1.0, got {instance.get('version')}"
    assert "$schema" in instance, "Missing $schema in SARIF root"
    runs = instance.get("runs")
    assert isinstance(runs, list) and len(runs) > 0, "SARIF runs must be a non-empty list"
    for r in runs:
        assert isinstance(r, dict), "Run must be an object"
        tool = r.get("tool")
        assert isinstance(tool, dict), "Run tool must be an object"
        driver = tool.get("driver")
        assert isinstance(driver, dict), "Tool driver must be an object"
        assert "name" in driver, "Driver must have a name"
        rules = driver.get("rules", [])
        assert isinstance(rules, list), "Driver rules must be a list"
        for rule in rules:
            assert "id" in rule, "Rule must have id"
            assert "shortDescription" in rule, "Rule must have shortDescription"
            assert "properties" in rule and "examined" in rule["properties"], "Rule must have properties.examined"
        invocations = r.get("invocations")
        assert isinstance(invocations, list) and len(invocations) > 0, "Run must have invocations list"
        inv = invocations[0]
        assert "executionSuccessful" in inv, "Invocation must have executionSuccessful"
        assert "ruleConfigurationOverrides" in inv, "Invocation must have ruleConfigurationOverrides"
        assert "properties" in inv, "Invocation must have properties"
        assert "totalExamined" in inv["properties"], "Invocation properties must have totalExamined"
        assert "totalOverrides" in inv["properties"], "Invocation properties must have totalOverrides"
        results = r.get("results")
        assert isinstance(results, list), "Results must be a list"
        for res in results:
            assert "ruleId" in res, "Result must have ruleId"
            assert "level" in res, "Result must have level"
            assert "message" in res, "Result must have message"
            assert "locations" in res, "Result must have locations"
    print(f"OK: {sarif_path} structurally validated against SARIF 2.1.0 (jsonschema not installed)")

def validate_junit(junit_path, xsd_path):
    xmllint = shutil.which("xmllint")
    if xmllint:
        res = subprocess.run([xmllint, "--noout", "--schema", xsd_path, junit_path], capture_output=True, text=True)
        if res.returncode != 0:
            sys.stderr.write(f"xmllint error:\n{res.stderr}\n{res.stdout}\n")
            sys.exit(res.returncode)
        print(f"OK: {junit_path} validates against {xsd_path} via xmllint")
        return
    try:
        import xmlschema
        xs = xmlschema.XMLSchema(xsd_path)
        xs.validate(junit_path)
        print(f"OK: {junit_path} validates against {xsd_path} via xmlschema")
    except ImportError:
        import xml.etree.ElementTree as ET
        tree = ET.parse(junit_path)
        root = tree.getroot()
        assert root.tag in ("testsuites", "testsuite")
        print(f"OK: {junit_path} parsed successfully by ElementTree")

if __name__ == "__main__":
    if len(sys.argv) < 4:
        sys.stderr.write("Usage: validate.py <sarif|junit> <file> <schema>\n")
        sys.exit(1)
    mode = sys.argv[1]
    target = sys.argv[2]
    schema = sys.argv[3]
    if mode == "sarif":
        validate_sarif(target, schema)
    elif mode == "junit":
        validate_junit(target, schema)
    else:
        sys.stderr.write(f"Unknown mode: {mode}\n")
        sys.exit(1)
