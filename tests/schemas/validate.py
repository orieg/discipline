#!/usr/bin/env python3
import sys
import json
import subprocess
import shutil

def validate_sarif(sarif_path, schema_path):
    import jsonschema
    with open(schema_path, "r", encoding="utf-8") as f:
        schema = json.load(f)
    with open(sarif_path, "r", encoding="utf-8") as f:
        instance = json.load(f)
    jsonschema.validate(instance=instance, schema=schema)
    print(f"OK: {sarif_path} validates against {schema_path}")

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
