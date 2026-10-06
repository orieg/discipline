//! `error-swallowing/error-replaced-by-constant` (#535): a new error handler that puts a
//! numeric literal in place of the result, in a file `constant_fallback_paths` names.
//!
//! Every fixture marks, in a comment on the handler's own line, what the gate must say
//! about it: `CONST` for the new finding, `DEFAULT` for a default literal the existing
//! `Empty Error Handler Added` reports. A handler with neither marker must not be reported.

mod common;
use common::{Repo, CONFIG_HEAD};

const CODE: &str = "error-swallowing/error-replaced-by-constant";
const TITLE: &str = "Error Replaced By Constant";
const EXISTING_TITLE: &str = "Empty Error Handler Added";

/// The table that turns the check on for `harness/**`.
const HARNESS_KEY: &str = "[gates.error-swallowing]\nconstant_fallback_paths = [\"harness/**\"]\n";

/// The issue's example.
const ISSUE_EXAMPLE: &str = "\
def measure(binary):
    try:
        ops_per_sec = run_arm(binary)
    except FileNotFoundError:
        ops_per_sec = 150000.0
    return ops_per_sec
";

/// The issue's example with a second handler for another error.
const TWO_HANDLERS: &str = "\
def measure(binary):
    try:
        ops_per_sec = run_arm(binary)
    except FileNotFoundError:
        ops_per_sec = 150000.0
    try:
        p99 = run_latency(binary)
    except TimeoutError:
        p99 = 1.5
    return ops_per_sec, p99
";

/// The issue's example with lines above it: the handler is the same one, further down.
const ISSUE_EXAMPLE_MOVED: &str = "\
import sys


def usage():
    return sys.argv[0]


def measure(binary):
    try:
        ops_per_sec = run_arm(binary)
    except FileNotFoundError:
        ops_per_sec = 150000.0
    return ops_per_sec
";

/// The issue's example with the inline marker on the handler's line.
const ISSUE_EXAMPLE_MARKED: &str = "\
def measure(binary):
    try:
        ops_per_sec = run_arm(binary)
    except FileNotFoundError:  # discipline:allow(error-swallowing): the arm is optional on this host
        ops_per_sec = 150000.0
    return ops_per_sec
";

const PYTHON: &str = "\
import math


def measure(binary, record, log):
    try:
        ops = run_arm(binary)
    except FileNotFoundError:  # CONST assignment to a name
        ops = 150000.0
    try:
        record.ops = run_arm(binary)
    except OSError:  # CONST attribute and subscript, logging beside
        log.warning(\"arm failed\")
        record.ops = -1
        record.raw[\"ops\"] = 2.5e5
    try:
        return run_arm(binary)
    except ValueError:  # CONST return of a number
        return 150000
    try:
        return run_all(binary)
    except KeyError:  # CONST dictionary of numbers
        return {\"ops\": 150000.0, \"p99\": 1.5}
    try:
        return run_all(binary)
    except IndexError:  # CONST list of numbers
        return [150000.0, -2]
    try:
        ops = run_arm(binary)
    except RuntimeError:  # re-raises
        ops = 150000.0
        raise
    try:
        ops = run_arm(binary)
    except TypeError as e:  # stores the error
        record.error = e
    try:
        ops = run_arm(binary)
    except LookupError as e:  # returns the error
        return e
    try:
        ops = run_arm(binary)
    except ArithmeticError:  # calls a function for the value
        ops = estimate(binary)
    try:
        ops = run_arm(binary)
    except EOFError:  # calls a function beside the number
        mark_failed(binary)
        ops = 150000.0
    try:
        ops = run_arm(binary)
    except MemoryError:  # absence: None
        ops = None
    try:
        ops = run_arm(binary)
    except BufferError:  # absence: a NaN built by a call
        ops = float(\"nan\")
    try:
        ops = run_arm(binary)
    except ImportError:  # absence: a NaN by name
        ops = math.nan
    try:
        ops = run_arm(binary)
    except NameError:  # the default literal, assigned
        ops = 0
    try:
        ops = run_arm(binary)
    except TimeoutError:  # a computed number
        ops = 150000.0 * 2
    try:
        ops = run_arm(binary)
    except ConnectionError:  # a named value
        ops = FALLBACK_OPS
    try:
        ops = run_arm(binary)
    except PermissionError:  # an update of what is there
        ops += 5
    try:
        return run_all(binary)
    except UnicodeError:  # CONST collection with a zero beside the number
        return [150000.0, 0]
    try:
        return run_all(binary)
    except FloatingPointError:  # a collection of zeros only
        return [0, 0.0]
    try:
        ops = run_arm(binary)
    except ZeroDivisionError:  # a zero written as a float
        ops = 0.0
    try:
        record.raw[slot(binary)] = run_arm(binary)
    except BlockingIOError:  # a call inside the target
        record.raw[slot(binary)] = 150000.0
    return ops
";

const PYTHON_DEFAULT: &str = "\
def measure(binary):
    try:
        return run_arm(binary)
    except AttributeError:  # DEFAULT
        return 0
";

const JS: &str = "\
export function measure(binary, record, logger) {
  let ops;
  try {
    ops = runArm(binary);
  } catch (e) { // CONST assignment to a name
    ops = 150000.0;
  }
  try {
    record.ops = runArm(binary);
  } catch (e) { // CONST member and subscript, logging beside
    console.error(\"arm failed\");
    record.ops = -1;
    record.raw[\"ops\"] = 2.5e5;
  }
  try {
    return runArm(binary);
  } catch (e) { // CONST return of a number
    return 150000;
  }
  try {
    return runAll(binary);
  } catch (e) { // CONST object of numbers
    return { ops: 150000.0, \"p99\": 1.5 };
  }
  try {
    return runAll(binary);
  } catch (e) { // CONST array of numbers
    return [150000.0, -2];
  }
  try {
    ops = runArm(binary);
  } catch (e) { // rethrows
    ops = 150000.0;
    throw e;
  }
  try {
    ops = runArm(binary);
  } catch (e) { // stores the error
    record.error = e;
  }
  try {
    ops = runArm(binary);
  } catch (e) { // returns the error
    return e;
  }
  try {
    ops = runArm(binary);
  } catch (e) { // calls a function for the value
    ops = estimate(binary);
  }
  try {
    ops = runArm(binary);
  } catch (e) { // calls a function beside the number
    markFailed(binary);
    ops = 150000.0;
  }
  try {
    ops = runArm(binary);
  } catch (e) { // absence: null
    ops = null;
  }
  try {
    ops = runArm(binary);
  } catch (e) { // absence: undefined
    ops = undefined;
  }
  try {
    ops = runArm(binary);
  } catch (e) { // absence: NaN
    ops = NaN;
  }
  try {
    ops = runArm(binary);
  } catch (e) { // absence: Number.NaN
    ops = Number.NaN;
  }
  try {
    ops = runArm(binary);
  } catch (e) { // the default literal, assigned
    ops = 0;
  }
  try {
    ops = runArm(binary);
  } catch (e) { // a computed number
    ops = 150000.0 * 2;
  }
  try {
    ops = runArm(binary);
  } catch (e) { // a named value
    ops = FALLBACK_OPS;
  }
  try {
    ops = runArm(binary);
  } catch (e) { // an update of what is there
    ops += 5;
  }
  try {
    ops = runArm(binary);
  } catch (e) { // a declaration local to the handler
    let local = 5;
  }
  try {
    return runAll(binary);
  } catch (e) { // CONST collection with a zero beside the number
    return [150000.0, 0];
  }
  try {
    return runAll(binary);
  } catch (e) { // a collection of zeros only
    return [0, 0.0];
  }
  try {
    ops = runArm(binary);
  } catch (e) { // a zero written as a float
    ops = 0.0;
  }
  return ops;
}
";

const JS_DEFAULT: &str = "\
export function measure(binary) {
  try {
    return runArm(binary);
  } catch (e) { // DEFAULT
    return 0;
  }
}
";

const JAVA: &str = "\
class Arm {
  double ops;
  Double boxed;
  double[] raw = new double[2];
  Exception lastError;

  double measure(String binary) {
    try {
      ops = runArm(binary);
    } catch (IOException e) { // CONST assignment to a name
      ops = 150000.0;
    }
    try {
      this.ops = runArm(binary);
    } catch (IOException e) { // CONST field and array element, logging beside
      System.err.println(\"arm failed\");
      this.ops = -1;
      raw[0] = 2.5e5;
    }
    try {
      return runArm(binary);
    } catch (IOException e) { // CONST return of a number
      return 150000L;
    }
    try {
      ops = runArm(binary);
    } catch (IOException e) { // rethrows
      ops = 150000.0;
      throw new IllegalStateException(e);
    }
    try {
      ops = runArm(binary);
    } catch (IOException e) { // stores the error
      lastError = e;
    }
    try {
      ops = runArm(binary);
    } catch (IOException e) { // calls a function for the value
      ops = estimate(binary);
    }
    try {
      ops = runArm(binary);
    } catch (IOException e) { // calls a function beside the number
      markFailed(binary);
      ops = 150000.0;
    }
    try {
      ops = runArm(binary);
    } catch (IOException e) { // absence: null
      boxed = null;
    }
    try {
      ops = runArm(binary);
    } catch (IOException e) { // absence: NaN
      ops = Double.NaN;
    }
    try {
      ops = runArm(binary);
    } catch (IOException e) { // the default literal, assigned
      ops = 0;
    }
    try {
      ops = runArm(binary);
    } catch (IOException e) { // a computed number
      ops = 150000.0 * 2;
    }
    try {
      ops = runArm(binary);
    } catch (IOException e) { // a named value
      ops = FALLBACK_OPS;
    }
    try {
      ops = runArm(binary);
    } catch (IOException e) { // an update of what is there
      ops += 5;
    }
    try {
      ops = runArm(binary);
    } catch (IOException e) { // a declaration local to the handler
      double local = 5;
    }
    return ops;
  }

  double[] all(String binary) {
    try {
      return runAll(binary);
    } catch (IOException e) { // CONST array of numbers
      return new double[] {150000.0, 1.5};
    }
  }

  Exception failure(String binary) {
    try {
      runArm(binary);
    } catch (IOException e) { // returns the error
      return e;
    }
    try {
      return runAll(binary);
    } catch (IOException e) { // a collection built by a call
      return List.of(150000.0, 1.5);
    }
  }
}
";

const JAVA_DEFAULT: &str = "\
class ArmDefault {
  double measure(String binary) {
    try {
      return runArm(binary);
    } catch (IOException e) { // DEFAULT
      return 0;
    }
  }
}
";

const PHP: &str = "\
<?php
function measure($binary, $record) {
    try {
        $ops = run_arm($binary);
    } catch (RuntimeException $e) { // CONST assignment to a name
        $ops = 150000.0;
    }
    try {
        $record->ops = run_arm($binary);
    } catch (RuntimeException $e) { // CONST property and subscript, logging beside
        error_log(\"arm failed\");
        $record->ops = -1;
        $record->raw['ops'] = 2.5e5;
    }
    try {
        return run_arm($binary);
    } catch (RuntimeException $e) { // CONST return of a number
        return 150000;
    }
    try {
        return run_all($binary);
    } catch (RuntimeException $e) { // CONST keyed array of numbers
        return ['ops' => 150000.0, 'p99' => 1.5];
    }
    try {
        return run_all($binary);
    } catch (RuntimeException $e) { // CONST array() of numbers
        return array(150000.0, -2);
    }
    try {
        $ops = run_arm($binary);
    } catch (RuntimeException $e) { // rethrows
        $ops = 150000.0;
        throw $e;
    }
    try {
        $ops = run_arm($binary);
    } catch (RuntimeException $e) { // stores the error
        $record->error = $e;
    }
    try {
        $ops = run_arm($binary);
    } catch (RuntimeException $e) { // returns the error
        return $e;
    }
    try {
        $ops = run_arm($binary);
    } catch (RuntimeException $e) { // calls a function for the value
        $ops = estimate($binary);
    }
    try {
        $ops = run_arm($binary);
    } catch (RuntimeException $e) { // calls a function beside the number
        mark_failed($binary);
        $ops = 150000.0;
    }
    try {
        $ops = run_arm($binary);
    } catch (RuntimeException $e) { // absence: null
        $ops = null;
    }
    try {
        $ops = run_arm($binary);
    } catch (RuntimeException $e) { // absence: NAN
        $ops = NAN;
    }
    try {
        $ops = run_arm($binary);
    } catch (RuntimeException $e) { // the default literal, assigned
        $ops = 0;
    }
    try {
        $ops = run_arm($binary);
    } catch (RuntimeException $e) { // a computed number
        $ops = 150000.0 * 2;
    }
    try {
        $ops = run_arm($binary);
    } catch (RuntimeException $e) { // a named value
        $ops = FALLBACK_OPS;
    }
    try {
        $ops = run_arm($binary);
    } catch (RuntimeException $e) { // an update of what is there
        $ops += 5;
    }
    try {
        return run_all($binary);
    } catch (RuntimeException $e) { // CONST collection with a zero beside the number
        return [150000.0, 0];
    }
    try {
        $ops = run_arm($binary);
    } catch (RuntimeException $e) { // a zero written as a float
        $ops = 0.0;
    }
    return $ops;
}
";

const PHP_DEFAULT: &str = "\
<?php
function measure_default($binary) {
    try {
        return run_arm($binary);
    } catch (RuntimeException $e) { // DEFAULT
        return 0;
    }
}
";

const RUBY: &str = "\
def measure(binary, record)
  begin
    ops = run_arm(binary)
  rescue Errno::ENOENT # CONST assignment to a name
    ops = 150000.0
  end
  begin
    record.ops = run_arm(binary)
  rescue IOError # CONST attribute, element and instance variable, logging beside
    puts \"arm failed\"
    record.ops = -1
    @raw[:ops] = 2.5e5
    @last = 3
  end
  begin
    return run_arm(binary)
  rescue ArgumentError # CONST return of a number
    return 150000
  end
  begin
    return run_all(binary)
  rescue KeyError # CONST hash of numbers
    return { ops: 150000.0, \"p99\" => 1.5 }
  end
  begin
    run_all(binary)
  rescue IndexError # CONST array as the value of the block
    [150000.0, -2]
  end
  begin
    run_arm(binary)
  rescue RangeError # CONST number as the value of the block
    150000.0
  end
  begin
    ops = run_arm(binary)
  rescue RuntimeError # re-raises
    ops = 150000.0
    raise
  end
  begin
    ops = run_arm(binary)
  rescue TypeError => e # stores the error
    @error = e
  end
  begin
    ops = run_arm(binary)
  rescue NameError => e # returns the error
    return e
  end
  begin
    ops = run_arm(binary)
  rescue ZeroDivisionError # calls a function for the value
    ops = estimate(binary)
  end
  begin
    ops = run_arm(binary)
  rescue EOFError # calls a function beside the number
    mark_failed(binary)
    ops = 150000.0
  end
  begin
    ops = run_arm(binary)
  rescue NoMemoryError # absence: nil
    ops = nil
  end
  begin
    ops = run_arm(binary)
  rescue FloatDomainError # absence: NaN
    ops = Float::NAN
  end
  begin
    ops = run_arm(binary)
  rescue ScriptError # a zero: nothing is reported, by this finding or the existing one
    ops = 0
  end
  begin
    ops = run_arm(binary)
  rescue Timeout::Error # a computed number
    ops = 150000.0 * 2
  end
  begin
    ops = run_arm(binary)
  rescue SystemCallError # a named value
    ops = FALLBACK_OPS
  end
  begin
    ops = run_arm(binary)
  rescue SecurityError # an update of what is there
    ops += 5
  end
  begin
    return run_all(binary)
  rescue EncodingError # a collection with an absence value in it
    return [150000.0, nil]
  end
  ops
end
";

const CPP: &str = "\
#include <cmath>
#include <iostream>
#include <stdexcept>
#include <string>
#include <vector>

double measure(const std::string& binary, Record& record, Record* out) {
  double ops = 0;
  try {
    ops = run_arm(binary);
  } catch (const std::runtime_error& e) { // CONST assignment to a name
    ops = 150000.0;
  }
  try {
    record.ops = run_arm(binary);
  } catch (const std::runtime_error& e) { // CONST field and subscript, logging beside
    std::cerr << \"arm failed\";
    record.ops = -1;
    out->raw[0] = 2.5e5;
  }
  try {
    return run_arm(binary);
  } catch (const std::runtime_error& e) { // CONST return of a number
    return 150000;
  }
  try {
    ops = run_arm(binary);
  } catch (const std::runtime_error& e) { // rethrows
    ops = 150000.0;
    throw;
  }
  try {
    ops = run_arm(binary);
  } catch (const std::runtime_error& e) { // stores the error
    record.error = e;
  }
  try {
    ops = run_arm(binary);
  } catch (const std::runtime_error& e) { // calls a function for the value
    ops = estimate(binary);
  }
  try {
    ops = run_arm(binary);
  } catch (const std::runtime_error& e) { // calls a function beside the number
    mark_failed(binary);
    ops = 150000.0;
  }
  try {
    ops = run_arm(binary);
  } catch (const std::runtime_error& e) { // absence: a null pointer
    out = nullptr;
  }
  try {
    ops = run_arm(binary);
  } catch (const std::runtime_error& e) { // absence: NAN
    ops = NAN;
  }
  try {
    ops = run_arm(binary);
  } catch (const std::runtime_error& e) { // absence: a NaN built by a call
    ops = std::nan(\"\");
  }
  try {
    ops = run_arm(binary);
  } catch (const std::runtime_error& e) { // the default literal, assigned
    ops = 0;
  }
  try {
    ops = run_arm(binary);
  } catch (const std::runtime_error& e) { // a computed number
    ops = 150000.0 * 2;
  }
  try {
    ops = run_arm(binary);
  } catch (const std::runtime_error& e) { // a named value
    ops = kFallbackOps;
  }
  try {
    ops = run_arm(binary);
  } catch (const std::runtime_error& e) { // an update of what is there
    ops += 5;
  }
  try {
    ops = run_arm(binary);
  } catch (const std::runtime_error& e) { // a declaration local to the handler
    double local = 5;
  }
  return ops;
}

std::vector<double> all(const std::string& binary) {
  try {
    return run_all(binary);
  } catch (const std::runtime_error& e) { // CONST initializer list of numbers
    return {150000.0, 1.5};
  }
}
";

const CPP_DEFAULT: &str = "\
double measure_default(const std::string& binary) {
  try {
    return run_arm(binary);
  } catch (const std::runtime_error& e) { // DEFAULT
    return 0;
  }
}
";

const KOTLIN: &str = "\
fun measure(binary: String, record: Record): Double {
    var ops = 0.0
    var boxed: Double? = 0.0
    try {
        ops = runArm(binary)
    } catch (e: IOException) { // CONST assignment to a name
        ops = 150000.0
    }
    try {
        record.ops = runArm(binary)
    } catch (e: IOException) { // CONST property and index, logging beside
        println(\"arm failed\")
        record.ops = -1.0
        record.raw[0] = 2.5e5
    }
    try {
        return runArm(binary)
    } catch (e: IOException) { // CONST return of a number
        return 150000.0
    }
    val viaExpression = try {
        runArm(binary)
    } catch (e: IOException) { // CONST number as the value of the try
        150000.0
    }
    try {
        ops = runArm(binary)
    } catch (e: IOException) { // rethrows
        ops = 150000.0
        throw e
    }
    try {
        ops = runArm(binary)
    } catch (e: IOException) { // stores the error
        record.error = e
    }
    try {
        ops = runArm(binary)
    } catch (e: IOException) { // calls a function for the value
        ops = estimate(binary)
    }
    try {
        ops = runArm(binary)
    } catch (e: IOException) { // calls a function beside the number
        markFailed(binary)
        ops = 150000.0
    }
    try {
        ops = runArm(binary)
    } catch (e: IOException) { // absence: null
        boxed = null
    }
    try {
        ops = runArm(binary)
    } catch (e: IOException) { // absence: NaN
        ops = Double.NaN
    }
    try {
        ops = runArm(binary)
    } catch (e: IOException) { // a zero: nothing is reported, by this finding or the existing one
        ops = 0.0
    }
    try {
        return runArm(binary)
    } catch (e: IOException) { // a zero returned: the same
        return 0.0
    }
    try {
        ops = runArm(binary)
    } catch (e: IOException) { // a computed number
        ops = 150000.0 * 2
    }
    try {
        ops = runArm(binary)
    } catch (e: IOException) { // a named value
        ops = FALLBACK_OPS
    }
    try {
        ops = runArm(binary)
    } catch (e: IOException) { // an update of what is there
        ops += 5.0
    }
    try {
        ops = runArm(binary)
    } catch (e: IOException) { // a declaration local to the handler
        val local = 5.0
    }
    return ops + viaExpression
}
";

const CSHARP: &str = "\
class Arm {
  double ops;
  double? boxed;
  double[] raw = new double[2];
  Exception lastError;

  double Measure(string binary) {
    try {
      ops = RunArm(binary);
    } catch (IOException e) { // CONST assignment to a name
      ops = 150000.0;
    }
    try {
      this.ops = RunArm(binary);
    } catch (IOException e) { // CONST member and element, logging beside
      Console.WriteLine(\"arm failed\");
      this.ops = -1;
      raw[0] = 2.5e5;
    }
    try {
      return RunArm(binary);
    } catch (IOException e) { // CONST return of a number
      return 150000;
    }
    try {
      ops = RunArm(binary);
    } catch (IOException e) { // rethrows
      ops = 150000.0;
      throw;
    }
    try {
      ops = RunArm(binary);
    } catch (IOException e) { // stores the error
      lastError = e;
    }
    try {
      ops = RunArm(binary);
    } catch (IOException e) { // calls a function for the value
      ops = Estimate(binary);
    }
    try {
      ops = RunArm(binary);
    } catch (IOException e) { // calls a function beside the number
      MarkFailed(binary);
      ops = 150000.0;
    }
    try {
      ops = RunArm(binary);
    } catch (IOException e) { // absence: null
      boxed = null;
    }
    try {
      ops = RunArm(binary);
    } catch (IOException e) { // absence: NaN
      ops = double.NaN;
    }
    try {
      ops = RunArm(binary);
    } catch (IOException e) { // the default literal, assigned
      ops = 0;
    }
    try {
      ops = RunArm(binary);
    } catch (IOException e) { // a computed number
      ops = 150000.0 * 2;
    }
    try {
      ops = RunArm(binary);
    } catch (IOException e) { // a named value
      ops = FallbackOps;
    }
    try {
      ops = RunArm(binary);
    } catch (IOException e) { // an update of what is there
      ops += 5;
    }
    try {
      ops = RunArm(binary);
    } catch (IOException e) { // a declaration local to the handler
      double local = 5;
    }
    return ops;
  }

  double[] All(string binary) {
    try {
      return RunAll(binary);
    } catch (IOException e) { // CONST array of numbers
      return new double[] {150000.0, 1.5};
    }
  }

  double[] AllImplicit(string binary) {
    try {
      return RunAll(binary);
    } catch (IOException e) { // CONST implicitly typed array of numbers
      return new[] {150000.0, 1.5};
    }
  }
}
";

const CSHARP_DEFAULT: &str = "\
class ArmDefault {
  double Measure(string binary) {
    try {
      return RunArm(binary);
    } catch (IOException e) { // DEFAULT
      return 0;
    }
  }
}
";

const SCALA: &str = "\
object Arm {
  def measure(binary: String, record: Record): Double = {
    var ops = 0.0
    try {
      ops = runArm(binary)
    } catch {
      case e: IOException => ops = 150000.0 // CONST assignment to a name
    }
    try {
      record.ops = runArm(binary)
    } catch {
      case e: IOException => // CONST field, logging beside
        println(\"arm failed\")
        record.ops = -1
    }
    val viaExpression = try {
      runArm(binary)
    } catch {
      case e: IOException => 150000.0 // CONST number as the value of the try
    }
    try {
      return runArm(binary)
    } catch {
      case e: IOException => return 150000.0 // CONST return of a number
    }
    try {
      ops = runArm(binary)
    } catch {
      case e: IllegalStateException => throw e
      case e: IllegalArgumentException =>
        ops = 150000.0
        throw e
      case e: ArithmeticException => record.error = e
      case e: ClassCastException => ops = estimate(binary)
      case e: IndexOutOfBoundsException =>
        markFailed(binary)
        ops = 150000.0
      case e: NullPointerException => ops = Double.NaN
      case e: NumberFormatException => ops = 0
      case e: SecurityException => ops = 150000.0 * 2
      case e: UnsupportedOperationException => ops = FallbackOps
      case e: InterruptedException => ops += 5
    }
    ops + viaExpression
  }
}
";

const SCALA_DEFAULT: &str = "\
object ArmDefault {
  def measure(binary: String): Double = {
    try {
      runArm(binary)
    } catch {
      case e: IOException => 0 // DEFAULT
    }
  }
}
";

const SWIFT: &str = "\
func measure(binary: String, record: Record) -> Double {
    var ops = 0.0
    var boxed: Double? = 0.0
    do {
        ops = try runArm(binary)
    } catch { // CONST assignment to a name
        ops = 150000.0
    }
    do {
        record.ops = try runArm(binary)
    } catch { // CONST property, logging beside
        print(\"arm failed\")
        record.ops = -1
    }
    do {
        return try runArm(binary)
    } catch { // CONST return of a number
        return 150000.0
    }
    do {
        ops = try runArm(binary)
    } catch { // rethrows
        ops = 150000.0
        throw error
    }
    do {
        ops = try runArm(binary)
    } catch { // stores the error
        record.error = error
    }
    do {
        ops = try runArm(binary)
    } catch { // calls a function for the value
        ops = estimate(binary)
    }
    do {
        ops = try runArm(binary)
    } catch { // calls a function beside the number
        markFailed(binary)
        ops = 150000.0
    }
    do {
        ops = try runArm(binary)
    } catch { // absence: nil
        boxed = nil
    }
    do {
        ops = try runArm(binary)
    } catch { // absence: NaN
        ops = Double.nan
    }
    do {
        ops = try runArm(binary)
    } catch { // a zero: nothing is reported, by this finding or the existing one
        ops = 0
    }
    do {
        return try runArm(binary)
    } catch { // a zero returned: the same
        return 0.0
    }
    do {
        ops = try runArm(binary)
    } catch { // a computed number
        ops = 150000.0 * 2
    }
    do {
        ops = try runArm(binary)
    } catch { // a named value
        ops = fallbackOps
    }
    do {
        ops = try runArm(binary)
    } catch { // an update of what is there
        ops += 5
    }
    do {
        ops = try runArm(binary)
    } catch { // a declaration local to the handler
        let local = 5.0
    }
    return ops
}

func all(binary: String) -> [Double] {
    do {
        return try runAll(binary)
    } catch { // CONST array of numbers
        return [150000.0, 1.5]
    }
}

func named(binary: String) -> [String: Double] {
    do {
        return try runNamed(binary)
    } catch { // CONST dictionary of numbers
        return [\"ops\": 150000.0, \"p99\": 1.5]
    }
}
";

const OBJC: &str = "\
#import <Foundation/Foundation.h>

double measure(NSString *binary, Record *record) {
  double ops = 0;
  id boxed = nil;
  @try {
    ops = runArm(binary);
  } @catch (NSException *e) { // CONST assignment to a name
    ops = 150000.0;
  }
  @try {
    record.ops = runArm(binary);
  } @catch (NSException *e) { // CONST property and element, logging beside
    NSLog(@\"arm failed\");
    record.ops = -1;
    record->raw[0] = 2.5e5;
  }
  @try {
    return runArm(binary);
  } @catch (NSException *e) { // CONST return of a number
    return 150000;
  }
  @try {
    ops = runArm(binary);
  } @catch (NSException *e) { // rethrows
    ops = 150000.0;
    @throw e;
  }
  @try {
    ops = runArm(binary);
  } @catch (NSException *e) { // stores the error
    record.error = e;
  }
  @try {
    ops = runArm(binary);
  } @catch (NSException *e) { // calls a function for the value
    ops = estimate(binary);
  }
  @try {
    ops = runArm(binary);
  } @catch (NSException *e) { // sends a message beside the number
    [record markFailed];
    ops = 150000.0;
  }
  @try {
    ops = runArm(binary);
  } @catch (NSException *e) { // absence: nil
    boxed = nil;
  }
  @try {
    ops = runArm(binary);
  } @catch (NSException *e) { // absence: NAN
    ops = NAN;
  }
  @try {
    ops = runArm(binary);
  } @catch (NSException *e) { // the default literal, assigned
    ops = 0;
  }
  @try {
    ops = runArm(binary);
  } @catch (NSException *e) { // a computed number
    ops = 150000.0 * 2;
  }
  @try {
    ops = runArm(binary);
  } @catch (NSException *e) { // a named value
    ops = kFallbackOps;
  }
  @try {
    ops = runArm(binary);
  } @catch (NSException *e) { // an update of what is there
    ops += 5;
  }
  return ops;
}
";

const OBJC_DEFAULT: &str = "\
double measureDefault(NSString *binary) {
  @try {
    return runArm(binary);
  } @catch (NSException *e) { // DEFAULT
    return 0;
  }
}
";

/// Go and Rust have no handler construct: the fallback is an ordinary branch, which the
/// issue lists under what the rule does not catch.
const GO_BRANCH: &str = "\
package harness

func Measure(binary string) float64 {
	ops, err := runArm(binary)
	if err != nil {
		ops = 150000.0
	}
	return ops
}
";

const RUST_BRANCH: &str = "\
pub fn measure(binary: &str) -> f64 {
    match run_arm(binary) {
        Ok(ops) => ops,
        Err(_) => 150000.0,
    }
}
";

/// Every pack with a handler construct: the path under `harness/` and its fixture.
const PACKS: &[(&str, &str)] = &[
    ("harness/arm.py", PYTHON),
    ("harness/arm.js", JS),
    ("harness/arm.ts", JS),
    ("harness/Arm.java", JAVA),
    ("harness/arm.php", PHP),
    ("harness/arm.rb", RUBY),
    ("harness/arm.cpp", CPP),
    ("harness/Arm.kt", KOTLIN),
    ("harness/Arm.cs", CSHARP),
    ("harness/Arm.scala", SCALA),
    ("harness/Arm.swift", SWIFT),
    ("harness/arm.m", OBJC),
];

/// The packs whose existing default-literal list holds a number (`return 0`, or a bare
/// `0` in Scala). Kotlin, Swift and Ruby list none.
const DEFAULTS: &[(&str, &str)] = &[
    ("harness/arm_default.py", PYTHON_DEFAULT),
    ("harness/arm_default.js", JS_DEFAULT),
    ("harness/arm_default.ts", JS_DEFAULT),
    ("harness/ArmDefault.java", JAVA_DEFAULT),
    ("harness/arm_default.php", PHP_DEFAULT),
    ("harness/arm_default.cpp", CPP_DEFAULT),
    ("harness/ArmDefault.cs", CSHARP_DEFAULT),
    ("harness/ArmDefault.scala", SCALA_DEFAULT),
    ("harness/arm_default.m", OBJC_DEFAULT),
];

/// A repository whose base configuration is `CONFIG_HEAD` followed by `table`.
fn repo_with(table: &str) -> Repo {
    let repo = Repo::new();
    repo.git(&["checkout", "-q", "main"]);
    repo.write("discipline.toml", &format!("{CONFIG_HEAD}{table}"));
    repo.commit("chore: config");
    repo.git(&["checkout", "-q", "-B", "work"]);
    repo
}

/// 1-based lines of `src` whose text holds `marker`.
fn marked(src: &str, marker: &str) -> Vec<usize> {
    src.lines()
        .enumerate()
        .filter(|(_, l)| l.contains(marker))
        .map(|(i, _)| i + 1)
        .collect()
}

/// `(line, code, severity)` of every `error-swallowing` finding in `file`, by line.
fn findings(run: &common::Run, file: &str) -> Vec<(u64, String, String)> {
    let mut got: Vec<(u64, String, String)> = run
        .violations("error-swallowing")
        .iter()
        .filter(|v| v["file"] == file)
        .map(|v| {
            (
                v["line"].as_u64().unwrap(),
                v["code"].as_str().unwrap().to_string(),
                v["severity"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    got.sort();
    got
}

fn constant_at(lines: &[usize]) -> Vec<(u64, String, String)> {
    lines
        .iter()
        .map(|l| (*l as u64, CODE.to_string(), "warning".to_string()))
        .collect()
}

#[test]
fn the_issue_example_is_a_warning_in_a_named_path_and_nothing_without_the_key() {
    // Control: with the key unset, the handler is not a site, as before this check.
    let repo = repo_with("");
    repo.write("harness/arm.py", ISSUE_EXAMPLE);
    repo.commit("feat: arm");
    let unset = repo.check(&[]);
    assert_eq!(unset.code, 0, "{}", unset.stdout);
    assert!(
        unset.violations("error-swallowing").is_empty(),
        "{}",
        unset.stdout
    );
    assert_eq!(unset.outcome("error-swallowing")["examined"], 0);

    let repo = repo_with(HARNESS_KEY);
    repo.write("harness/arm.py", ISSUE_EXAMPLE);
    repo.commit("feat: arm");
    let run = repo.check(&[]);
    // The gate is at its default severity, `error`; the finding is held to a warning.
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);
    assert_eq!(findings(&run, "harness/arm.py"), constant_at(&[4]));
    let v = &run.violations("error-swallowing")[0];
    assert_eq!(v["title"], TITLE);
    let message = v["message"].as_str().unwrap();
    assert!(
        message.contains("except FileNotFoundError:") && message.contains("numeric literal"),
        "{message}"
    );
    assert_eq!(run.json()["errors"], 0);
    assert_eq!(run.json()["warnings"], 1);
    assert_eq!(run.outcome("error-swallowing")["examined"], 1);

    // A gate set below a warning keeps its own severity: the cap only lowers.
    let repo = repo_with(&format!("{HARNESS_KEY}severity = \"note\"\n"));
    repo.write("harness/arm.py", ISSUE_EXAMPLE);
    repo.commit("feat: arm");
    let note = repo.check(&[]);
    assert_eq!(
        findings(&note, "harness/arm.py"),
        vec![(4, CODE.to_string(), "note".to_string())]
    );
}

#[test]
fn every_pack_with_a_handler_reports_the_marked_handlers_and_no_other() {
    let repo = repo_with(HARNESS_KEY);
    for (path, src) in PACKS {
        repo.write(path, src);
    }
    repo.write("harness/arm.go", GO_BRANCH);
    repo.write("harness/arm.rs", RUST_BRANCH);
    repo.commit("feat: arms");
    let run = repo.check(&[]);
    for (path, src) in PACKS {
        let expected = marked(src, "CONST");
        assert!(
            expected.len() >= 3,
            "{path}: the fixture marks {expected:?}"
        );
        assert_eq!(
            findings(&run, path),
            constant_at(&expected),
            "{path}\n{}",
            run.stdout
        );
    }
    // Every finding of the gate is one of the marked handlers: nothing in the Go and Rust
    // files, and nothing at `error`.
    let total: usize = PACKS
        .iter()
        .map(|(_, src)| marked(src, "CONST").len())
        .sum();
    assert_eq!(run.violations("error-swallowing").len(), total);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(!run.outcome("error-swallowing")["notes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n.as_str().unwrap().contains("not analysed")));
}

#[test]
fn a_default_literal_is_the_existing_finding_and_is_not_reported_twice() {
    let repo = repo_with(HARNESS_KEY);
    for (path, src) in DEFAULTS {
        repo.write(path, src);
    }
    repo.commit("feat: arms");
    let run = repo.check(&[]);
    assert_eq!(run.code, 1, "{}", run.stdout);
    for (path, src) in DEFAULTS {
        let line = marked(src, "DEFAULT");
        assert_eq!(line.len(), 1, "{path}");
        assert_eq!(
            findings(&run, path),
            vec![(
                line[0] as u64,
                "error-swallowing/empty-error-handler-added".to_string(),
                "error".to_string()
            )],
            "{path}\n{}",
            run.stdout
        );
    }
    assert!(run
        .titles("error-swallowing")
        .iter()
        .all(|t| t == EXISTING_TITLE));
}

#[test]
fn a_file_outside_the_paths_and_a_test_file_inside_them_are_not_judged() {
    let repo = repo_with(HARNESS_KEY);
    // Outside `harness/**`: a numeric fallback is ordinary in application code.
    repo.write("src/arm.py", ISSUE_EXAMPLE);
    // Inside, but test code by its file name and by its directory: the gate's test-scope
    // rule holds for this finding as for the others.
    repo.write("harness/test_arm.py", ISSUE_EXAMPLE);
    repo.write("harness/tests/arm.py", ISSUE_EXAMPLE);
    // Positive control: the same text in a production file inside the paths.
    repo.write("harness/arm.py", ISSUE_EXAMPLE);
    repo.commit("feat: arms");
    let run = repo.check(&[]);
    let files: Vec<String> = run
        .violations("error-swallowing")
        .iter()
        .map(|v| v["file"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(files, vec!["harness/arm.py".to_string()], "{}", run.stdout);
}

#[test]
fn a_handler_that_only_moved_is_not_new_and_one_added_beside_it_is() {
    let repo = repo_with(HARNESS_KEY);
    repo.commit_base("harness/arm.py", ISSUE_EXAMPLE, "feat: arm");
    repo.write("harness/arm.py", ISSUE_EXAMPLE_MOVED);
    repo.commit("refactor: usage");
    let moved = repo.check(&[]);
    assert!(
        moved.violations("error-swallowing").is_empty(),
        "{}",
        moved.stdout
    );
    // The handler was looked at on the head side.
    assert_eq!(moved.outcome("error-swallowing")["examined"], 1);

    // Positive control: a second handler beside the one the base already had.
    repo.write("harness/arm.py", TWO_HANDLERS);
    repo.commit("feat: latency");
    let added = repo.check(&[]);
    assert_eq!(findings(&added, "harness/arm.py"), constant_at(&[8]));
}

#[test]
fn allow_swallow_lifts_it_by_path_by_path_and_line_and_by_the_inline_marker() {
    // By path: both handlers of the file.
    let repo = repo_with(HARNESS_KEY);
    repo.write("harness/arm.py", TWO_HANDLERS);
    repo.commit("feat: arm\n\nallow-swallow: harness/arm.py the figures are placeholders until the arm lands");
    let by_path = repo.check(&[]);
    assert!(
        by_path.violations("error-swallowing").is_empty(),
        "{}",
        by_path.stdout
    );
    let overrides = by_path.outcome("error-swallowing")["overrides"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(overrides.len(), 2, "{overrides:?}");
    assert_eq!(by_path.json()["warnings"], 0);

    // By `path:line`: that handler only.
    let repo = repo_with(HARNESS_KEY);
    repo.write("harness/arm.py", TWO_HANDLERS);
    repo.commit(
        "feat: arm\n\nallow-swallow: harness/arm.py:4 the throughput arm is optional on this host",
    );
    let by_line = repo.check(&[]);
    assert_eq!(findings(&by_line, "harness/arm.py"), constant_at(&[8]));
    assert_eq!(
        by_line.outcome("error-swallowing")["overrides"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // Control: a directive that names another file, with or without a line, lifts nothing.
    for subject in ["harness/other.py", "harness/other.py:4"] {
        let repo = repo_with(HARNESS_KEY);
        repo.write("harness/arm.py", TWO_HANDLERS);
        repo.commit(&format!(
            "feat: arm\n\nallow-swallow: {subject} the figures are placeholders"
        ));
        let other = repo.check(&[]);
        assert_eq!(
            findings(&other, "harness/arm.py"),
            constant_at(&[4, 8]),
            "{subject}"
        );
    }

    // The inline marker on the handler's line.
    let repo = repo_with(HARNESS_KEY);
    repo.write("harness/arm.py", ISSUE_EXAMPLE_MARKED);
    repo.commit("feat: arm");
    let inline = repo.check(&[]);
    assert!(
        inline.violations("error-swallowing").is_empty(),
        "{}",
        inline.stdout
    );
    assert_eq!(inline.outcome("error-swallowing")["inline_exemptions"], 1);
}

#[test]
fn an_invalid_glob_in_the_key_stops_the_run_and_names_the_key() {
    let repo = repo_with("[gates.error-swallowing]\nconstant_fallback_paths = [\"harness/[\"]\n");
    repo.write("harness/arm.py", ISSUE_EXAMPLE);
    repo.commit("feat: arm");
    let run = repo.check(&[]);
    assert_eq!(run.code, 2, "{}\n{}", run.stdout, run.stderr);
    // As for the gate's other glob lists: the run stops on the gate that holds the key.
    assert_eq!(
        run.could_not_check(),
        ("gate".to_string(), Some("error-swallowing".to_string()))
    );
    let detail = run.json()["could_not_check"]["detail"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        detail.contains("invalid glob `harness/[`")
            && detail.contains("gates.error-swallowing.constant_fallback_paths"),
        "{detail}"
    );
}

#[test]
fn config_integrity_reports_an_entry_removed_from_the_key_and_not_one_added() {
    let two = "[gates.error-swallowing]\nconstant_fallback_paths = [\"harness/**\", \"eval/**\"]\n";
    let weakened = |head_table: &str| {
        let repo = repo_with(two);
        repo.write("discipline.toml", &format!("{CONFIG_HEAD}{head_table}"));
        repo.commit("chore: tune");
        let run = repo.check(&[]);
        let messages: Vec<String> = run
            .violations("config-integrity")
            .iter()
            .map(|v| v["message"].as_str().unwrap().to_string())
            .collect();
        (run.titles("config-integrity"), messages)
    };

    // An entry removed: the check no longer reaches `eval/**`.
    let (titles, messages) = weakened(HARNESS_KEY);
    assert_eq!(titles, vec!["Gate Weakened By This Change"]);
    assert!(
        messages[0].contains("`constant_fallback_paths`"),
        "{messages:?}"
    );

    // The key removed: the check is off again.
    let (titles, messages) = weakened("");
    assert_eq!(titles, vec!["Gate Weakened By This Change"], "{messages:?}");

    // An entry added: the check reaches more, which is not a weakening.
    let (titles, messages) = weakened(
        "[gates.error-swallowing]\nconstant_fallback_paths = [\"harness/**\", \"eval/**\", \"scripts/bench/**\"]\n",
    );
    assert!(titles.is_empty(), "{messages:?}");

    // The key set where the base had none: not a weakening either.
    let repo = repo_with("");
    repo.write("discipline.toml", &format!("{CONFIG_HEAD}{HARNESS_KEY}"));
    repo.commit("chore: name the harness");
    let run = repo.check(&[]);
    assert!(run.titles("config-integrity").is_empty(), "{}", run.stdout);
}

#[test]
fn two_findings_in_one_file_have_their_own_lines_and_fingerprints() {
    let repo = repo_with(HARNESS_KEY);
    repo.write("harness/arm.py", TWO_HANDLERS);
    repo.commit("feat: arm");
    let run = repo.check(&[]);
    let found = run.violations("error-swallowing");
    assert_eq!(found.len(), 2, "{}", run.stdout);
    let prints: Vec<&str> = found
        .iter()
        .map(|v| v["fingerprint"].as_str().unwrap())
        .collect();
    assert!(prints.iter().all(|p| p.len() == 64), "{prints:?}");
    assert_ne!(prints[0], prints[1]);
    // A line, so no anchor is needed (docs/ARCHITECTURE.md, the anchor rule).
    assert!(found.iter().all(|v| v["line"].as_u64().is_some()));
    assert!(found.iter().all(|v| v["anchor"].is_null()));
}

/// Two handlers that swallow, the gate's oldest finding.
const TWO_EMPTY_HANDLERS: &str = "\
def load(p):
    try:
        a = open(p).read()
    except FileNotFoundError:
        pass
    try:
        b = open(p + '.bak').read()
    except PermissionError:
        pass
    return a, b
";

const TWO_EMPTY_HANDLERS_ONE_MARKED: &str = "\
def load(p):
    try:
        a = open(p).read()
    except FileNotFoundError:  # discipline:allow(error-swallowing): a missing file is the first run
        pass
    try:
        b = open(p + '.bak').read()
    except PermissionError:
        pass
    return a, b
";

/// `(lines still reported, overrides, unused `allow-swallow` directives)` for
/// `pkg/io.py` holding `src`, with `directive` in the commit message.
fn lifted_by(src: &str, directive: &str) -> (Vec<u64>, usize, usize) {
    let repo = Repo::new();
    repo.write("pkg/io.py", src);
    repo.commit(&format!("feat: io\n\n{directive}"));
    let run = repo.check(&[]);
    let lines = findings(&run, "pkg/io.py")
        .into_iter()
        .map(|(line, _, _)| line)
        .collect();
    let overrides = run.outcome("error-swallowing")["overrides"]
        .as_array()
        .unwrap()
        .len();
    let unused = run.json()["unused_directives"].as_array().map_or(0, |u| {
        u.iter()
            .filter(|d| d["directive"] == "allow-swallow")
            .count()
    });
    (lines, overrides, unused)
}

/// A subject written as `path:line` names the finding on that line, for the gate's
/// existing findings as for the new one; before this change it lifted the whole file.
#[test]
fn a_subject_with_a_line_lifts_that_line_only_and_a_bare_path_the_file() {
    // The handler on line 4 is lifted; the one on line 8 is still reported.
    assert_eq!(
        lifted_by(
            TWO_EMPTY_HANDLERS,
            "allow-swallow: pkg/io.py:4 a missing file is the first run"
        ),
        (vec![8], 1, 0)
    );
    // Control: a bare path lifts both, as before.
    assert_eq!(
        lifted_by(
            TWO_EMPTY_HANDLERS,
            "allow-swallow: pkg/io.py both files are optional"
        ),
        (vec![], 2, 0)
    );
    // Control: so does a bare file name.
    assert_eq!(
        lifted_by(
            TWO_EMPTY_HANDLERS,
            "allow-swallow: io.py both files are optional"
        ),
        (vec![], 2, 0)
    );
    // A line where no finding sits lifts nothing, and the directive is reported unused.
    assert_eq!(
        lifted_by(
            TWO_EMPTY_HANDLERS,
            "allow-swallow: pkg/io.py:9 a missing file is the first run"
        ),
        (vec![4, 8], 0, 1)
    );
    // Nor does a file name with a line fall back to the whole file.
    assert_eq!(
        lifted_by(
            TWO_EMPTY_HANDLERS,
            "allow-swallow: io.py:9 a missing file is the first run"
        ),
        (vec![4, 8], 0, 1)
    );
    // The inline marker is unchanged: its own line only.
    let repo = Repo::new();
    repo.write("pkg/io.py", TWO_EMPTY_HANDLERS_ONE_MARKED);
    repo.commit("feat: io");
    let run = repo.check(&[]);
    assert_eq!(
        findings(&run, "pkg/io.py")
            .into_iter()
            .map(|(line, _, _)| line)
            .collect::<Vec<_>>(),
        vec![8]
    );
    assert_eq!(run.outcome("error-swallowing")["inline_exemptions"], 1);
}
