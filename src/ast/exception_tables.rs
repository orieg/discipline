//! Standard exception class hierarchies, one `(class, direct parent)` pair per entry.
//!
//! Each table is pinned to a fixture under `tests/fixtures/exception_hierarchy/`, which a
//! runtime produced (the fixture's header names the runtime and the command); the .NET
//! fixture was entered from the API reference. A class a language does not ship is in no
//! table. `expected_exceptions` reads them.

/// `(class, direct parent)` pairs. A class with two parents has two entries.
pub(super) type Hierarchy = &'static [(&'static str, &'static str)];

/// Python built-in exceptions.
pub(super) const PYTHON: Hierarchy = &[
    ("ArithmeticError", "Exception"),
    ("AssertionError", "Exception"),
    ("AttributeError", "Exception"),
    ("BaseExceptionGroup", "BaseException"),
    ("BlockingIOError", "OSError"),
    ("BrokenPipeError", "ConnectionError"),
    ("BufferError", "Exception"),
    ("BytesWarning", "Warning"),
    ("ChildProcessError", "OSError"),
    ("ConnectionAbortedError", "ConnectionError"),
    ("ConnectionError", "OSError"),
    ("ConnectionRefusedError", "ConnectionError"),
    ("ConnectionResetError", "ConnectionError"),
    ("DeprecationWarning", "Warning"),
    ("EOFError", "Exception"),
    ("EncodingWarning", "Warning"),
    ("Exception", "BaseException"),
    ("ExceptionGroup", "BaseExceptionGroup"),
    ("ExceptionGroup", "Exception"),
    ("FileExistsError", "OSError"),
    ("FileNotFoundError", "OSError"),
    ("FloatingPointError", "ArithmeticError"),
    ("FutureWarning", "Warning"),
    ("GeneratorExit", "BaseException"),
    ("ImportError", "Exception"),
    ("ImportWarning", "Warning"),
    ("IndentationError", "SyntaxError"),
    ("IndexError", "LookupError"),
    ("InterruptedError", "OSError"),
    ("IsADirectoryError", "OSError"),
    ("KeyError", "LookupError"),
    ("KeyboardInterrupt", "BaseException"),
    ("LookupError", "Exception"),
    ("MemoryError", "Exception"),
    ("ModuleNotFoundError", "ImportError"),
    ("NameError", "Exception"),
    ("NotADirectoryError", "OSError"),
    ("NotImplementedError", "RuntimeError"),
    ("OSError", "Exception"),
    ("OverflowError", "ArithmeticError"),
    ("PendingDeprecationWarning", "Warning"),
    ("PermissionError", "OSError"),
    ("ProcessLookupError", "OSError"),
    ("PythonFinalizationError", "RuntimeError"),
    ("RecursionError", "RuntimeError"),
    ("ReferenceError", "Exception"),
    ("ResourceWarning", "Warning"),
    ("RuntimeError", "Exception"),
    ("RuntimeWarning", "Warning"),
    ("StopAsyncIteration", "Exception"),
    ("StopIteration", "Exception"),
    ("SyntaxError", "Exception"),
    ("SyntaxWarning", "Warning"),
    ("SystemError", "Exception"),
    ("SystemExit", "BaseException"),
    ("TabError", "IndentationError"),
    ("TimeoutError", "OSError"),
    ("TypeError", "Exception"),
    ("UnboundLocalError", "NameError"),
    ("UnicodeDecodeError", "UnicodeError"),
    ("UnicodeEncodeError", "UnicodeError"),
    ("UnicodeError", "ValueError"),
    ("UnicodeTranslateError", "UnicodeError"),
    ("UnicodeWarning", "Warning"),
    ("UserWarning", "Warning"),
    ("ValueError", "Exception"),
    ("Warning", "Exception"),
    ("ZeroDivisionError", "ArithmeticError"),
];

/// Java exceptions of `java.lang`, `java.io`, `java.net`, `java.nio`, `java.util`, `java.time` and the
/// packages below them that the fixture names.
pub(super) const JAVA: Hierarchy = &[
    ("AbstractMethodError", "IncompatibleClassChangeError"),
    ("AccessDeniedException", "FileSystemException"),
    ("ArithmeticException", "RuntimeException"),
    (
        "ArrayIndexOutOfBoundsException",
        "IndexOutOfBoundsException",
    ),
    ("ArrayStoreException", "RuntimeException"),
    ("AssertionError", "Error"),
    ("AtomicMoveNotSupportedException", "FileSystemException"),
    ("BindException", "SocketException"),
    ("BootstrapMethodError", "LinkageError"),
    ("BrokenBarrierException", "Exception"),
    ("BufferOverflowException", "RuntimeException"),
    ("BufferUnderflowException", "RuntimeException"),
    ("CancellationException", "IllegalStateException"),
    ("CharConversionException", "IOException"),
    ("CharacterCodingException", "IOException"),
    ("ClassCastException", "RuntimeException"),
    ("ClassCircularityError", "LinkageError"),
    ("ClassFormatError", "LinkageError"),
    ("ClassNotFoundException", "ReflectiveOperationException"),
    ("CloneNotSupportedException", "Exception"),
    ("ClosedDirectoryStreamException", "IllegalStateException"),
    ("ClosedFileSystemException", "IllegalStateException"),
    ("ClosedWatchServiceException", "IllegalStateException"),
    ("CoderMalfunctionError", "Error"),
    ("CompletionException", "RuntimeException"),
    ("ConcurrentModificationException", "RuntimeException"),
    ("ConnectException", "SocketException"),
    ("DataFormatException", "Exception"),
    ("DateTimeException", "RuntimeException"),
    ("DateTimeParseException", "DateTimeException"),
    (
        "DirectoryIteratorException",
        "ConcurrentModificationException",
    ),
    ("DirectoryNotEmptyException", "FileSystemException"),
    ("DuplicateFormatFlagsException", "IllegalFormatException"),
    ("EOFException", "IOException"),
    ("EmptyStackException", "RuntimeException"),
    ("EnumConstantNotPresentException", "RuntimeException"),
    ("Error", "Throwable"),
    ("Exception", "Throwable"),
    ("ExceptionInInitializerError", "LinkageError"),
    ("ExecutionException", "Exception"),
    ("FileAlreadyExistsException", "FileSystemException"),
    ("FileNotFoundException", "IOException"),
    ("FileSystemAlreadyExistsException", "RuntimeException"),
    ("FileSystemException", "IOException"),
    ("FileSystemLoopException", "FileSystemException"),
    ("FileSystemNotFoundException", "RuntimeException"),
    (
        "FormatFlagsConversionMismatchException",
        "IllegalFormatException",
    ),
    ("FormatterClosedException", "IllegalStateException"),
    ("GenericSignatureFormatError", "ClassFormatError"),
    ("HttpRetryException", "IOException"),
    ("IOError", "Error"),
    ("IOException", "Exception"),
    ("IllegalAccessError", "IncompatibleClassChangeError"),
    ("IllegalAccessException", "ReflectiveOperationException"),
    ("IllegalArgumentException", "RuntimeException"),
    ("IllegalCallerException", "RuntimeException"),
    ("IllegalCharsetNameException", "IllegalArgumentException"),
    ("IllegalFormatCodePointException", "IllegalFormatException"),
    ("IllegalFormatConversionException", "IllegalFormatException"),
    ("IllegalFormatException", "IllegalArgumentException"),
    ("IllegalFormatFlagsException", "IllegalFormatException"),
    ("IllegalFormatPrecisionException", "IllegalFormatException"),
    ("IllegalFormatWidthException", "IllegalFormatException"),
    ("IllegalMonitorStateException", "RuntimeException"),
    ("IllegalStateException", "RuntimeException"),
    ("IllegalThreadStateException", "IllegalArgumentException"),
    ("IllformedLocaleException", "RuntimeException"),
    ("InaccessibleObjectException", "RuntimeException"),
    ("IncompatibleClassChangeError", "LinkageError"),
    ("IndexOutOfBoundsException", "RuntimeException"),
    ("InputMismatchException", "NoSuchElementException"),
    ("InstantiationError", "IncompatibleClassChangeError"),
    ("InstantiationException", "ReflectiveOperationException"),
    ("InternalError", "VirtualMachineError"),
    ("InterruptedException", "Exception"),
    ("InterruptedIOException", "IOException"),
    ("InvalidClassException", "ObjectStreamException"),
    ("InvalidMarkException", "IllegalStateException"),
    ("InvalidObjectException", "ObjectStreamException"),
    ("InvalidPathException", "IllegalArgumentException"),
    ("InvalidPropertiesFormatException", "IOException"),
    ("InvocationTargetException", "ReflectiveOperationException"),
    ("LayerInstantiationException", "RuntimeException"),
    ("LinkageError", "Error"),
    ("MalformedInputException", "CharacterCodingException"),
    ("MalformedParameterizedTypeException", "RuntimeException"),
    ("MalformedParametersException", "RuntimeException"),
    ("MalformedURLException", "IOException"),
    ("MissingFormatArgumentException", "IllegalFormatException"),
    ("MissingFormatWidthException", "IllegalFormatException"),
    ("MissingResourceException", "RuntimeException"),
    ("NegativeArraySizeException", "RuntimeException"),
    ("NoClassDefFoundError", "LinkageError"),
    ("NoRouteToHostException", "SocketException"),
    ("NoSuchElementException", "RuntimeException"),
    ("NoSuchFieldError", "IncompatibleClassChangeError"),
    ("NoSuchFieldException", "ReflectiveOperationException"),
    ("NoSuchFileException", "FileSystemException"),
    ("NoSuchMethodError", "IncompatibleClassChangeError"),
    ("NoSuchMethodException", "ReflectiveOperationException"),
    ("NotActiveException", "ObjectStreamException"),
    ("NotDirectoryException", "FileSystemException"),
    ("NotLinkException", "FileSystemException"),
    ("NotSerializableException", "ObjectStreamException"),
    ("NullPointerException", "RuntimeException"),
    ("NumberFormatException", "IllegalArgumentException"),
    ("ObjectStreamException", "IOException"),
    ("OptionalDataException", "ObjectStreamException"),
    ("OutOfMemoryError", "VirtualMachineError"),
    ("PatternSyntaxException", "IllegalArgumentException"),
    ("PortUnreachableException", "SocketException"),
    ("ProtocolException", "IOException"),
    ("ProviderMismatchException", "IllegalArgumentException"),
    ("ProviderNotFoundException", "RuntimeException"),
    ("ReadOnlyBufferException", "UnsupportedOperationException"),
    (
        "ReadOnlyFileSystemException",
        "UnsupportedOperationException",
    ),
    ("ReflectiveOperationException", "Exception"),
    ("RejectedExecutionException", "RuntimeException"),
    ("RuntimeException", "Exception"),
    ("SecurityException", "RuntimeException"),
    ("ServiceConfigurationError", "Error"),
    ("SocketException", "IOException"),
    ("SocketTimeoutException", "InterruptedIOException"),
    ("StackOverflowError", "VirtualMachineError"),
    ("StreamCorruptedException", "ObjectStreamException"),
    (
        "StringIndexOutOfBoundsException",
        "IndexOutOfBoundsException",
    ),
    ("SyncFailedException", "IOException"),
    ("ThreadDeath", "Error"),
    ("TimeoutException", "Exception"),
    ("TooManyListenersException", "Exception"),
    ("TypeNotPresentException", "RuntimeException"),
    ("URISyntaxException", "Exception"),
    ("UTFDataFormatException", "IOException"),
    ("UncheckedIOException", "RuntimeException"),
    ("UndeclaredThrowableException", "RuntimeException"),
    ("UnknownError", "VirtualMachineError"),
    ("UnknownFormatConversionException", "IllegalFormatException"),
    ("UnknownFormatFlagsException", "IllegalFormatException"),
    ("UnknownHostException", "IOException"),
    ("UnknownServiceException", "IOException"),
    ("UnmappableCharacterException", "CharacterCodingException"),
    ("UnsatisfiedLinkError", "LinkageError"),
    ("UnsupportedCharsetException", "IllegalArgumentException"),
    ("UnsupportedClassVersionError", "ClassFormatError"),
    ("UnsupportedEncodingException", "IOException"),
    ("UnsupportedOperationException", "RuntimeException"),
    ("VerifyError", "LinkageError"),
    ("VirtualMachineError", "Error"),
    ("WriteAbortedException", "ObjectStreamException"),
    ("ZipError", "InternalError"),
    ("ZipException", "IOException"),
];

/// .NET exceptions of `System`, `System.IO`, `System.Collections.Generic` and
/// `System.Threading.Tasks`.
pub(super) const DOTNET: Hierarchy = &[
    ("AggregateException", "Exception"),
    ("ApplicationException", "Exception"),
    ("ArgumentException", "SystemException"),
    ("ArgumentNullException", "ArgumentException"),
    ("ArgumentOutOfRangeException", "ArgumentException"),
    ("ArithmeticException", "SystemException"),
    ("ArrayTypeMismatchException", "SystemException"),
    ("DirectoryNotFoundException", "IOException"),
    ("DivideByZeroException", "ArithmeticException"),
    ("DllNotFoundException", "TypeLoadException"),
    ("DriveNotFoundException", "IOException"),
    ("EndOfStreamException", "IOException"),
    ("EntryPointNotFoundException", "TypeLoadException"),
    ("FieldAccessException", "MemberAccessException"),
    ("FileLoadException", "IOException"),
    ("FileNotFoundException", "IOException"),
    ("FormatException", "SystemException"),
    ("IOException", "SystemException"),
    ("IndexOutOfRangeException", "SystemException"),
    ("InsufficientMemoryException", "OutOfMemoryException"),
    ("InvalidCastException", "SystemException"),
    ("InvalidOperationException", "SystemException"),
    ("KeyNotFoundException", "SystemException"),
    ("MemberAccessException", "SystemException"),
    ("MethodAccessException", "MemberAccessException"),
    ("MissingFieldException", "MissingMemberException"),
    ("MissingMemberException", "MemberAccessException"),
    ("MissingMethodException", "MissingMemberException"),
    ("NotFiniteNumberException", "ArithmeticException"),
    ("NotImplementedException", "SystemException"),
    ("NotSupportedException", "SystemException"),
    ("NullReferenceException", "SystemException"),
    ("ObjectDisposedException", "InvalidOperationException"),
    ("OperationCanceledException", "SystemException"),
    ("OutOfMemoryException", "SystemException"),
    ("OverflowException", "ArithmeticException"),
    ("PathTooLongException", "IOException"),
    ("PlatformNotSupportedException", "NotSupportedException"),
    ("RankException", "SystemException"),
    ("StackOverflowException", "SystemException"),
    ("SystemException", "Exception"),
    ("TaskCanceledException", "OperationCanceledException"),
    ("TimeoutException", "SystemException"),
    ("TypeLoadException", "SystemException"),
    ("UnauthorizedAccessException", "SystemException"),
    ("UriFormatException", "FormatException"),
];

/// JavaScript native errors, and the host's `DOMException`.
pub(super) const JS: Hierarchy = &[
    ("AggregateError", "Error"),
    ("DOMException", "Error"),
    ("EvalError", "Error"),
    ("RangeError", "Error"),
    ("ReferenceError", "Error"),
    ("SuppressedError", "Error"),
    ("SyntaxError", "Error"),
    ("TypeError", "Error"),
    ("URIError", "Error"),
];

/// PHP predefined, SPL and JSON exceptions.
pub(super) const PHP: Hierarchy = &[
    ("ArgumentCountError", "TypeError"),
    ("ArithmeticError", "Error"),
    ("AssertionError", "Error"),
    ("BadFunctionCallException", "LogicException"),
    ("BadMethodCallException", "BadFunctionCallException"),
    ("ClosedGeneratorException", "Exception"),
    ("CompileError", "Error"),
    ("DivisionByZeroError", "ArithmeticError"),
    ("DomainException", "LogicException"),
    ("Error", "Throwable"),
    ("ErrorException", "Exception"),
    ("Exception", "Throwable"),
    ("FiberError", "Error"),
    ("InvalidArgumentException", "LogicException"),
    ("JsonException", "Exception"),
    ("LengthException", "LogicException"),
    ("LogicException", "Exception"),
    ("OutOfBoundsException", "RuntimeException"),
    ("OutOfRangeException", "LogicException"),
    ("OverflowException", "RuntimeException"),
    ("ParseError", "CompileError"),
    ("RangeException", "RuntimeException"),
    ("RequestParseBodyException", "Exception"),
    ("RuntimeException", "Exception"),
    ("TypeError", "Error"),
    ("UnderflowException", "RuntimeException"),
    ("UnexpectedValueException", "RuntimeException"),
    ("UnhandledMatchError", "Error"),
    ("ValueError", "Error"),
];

/// Ruby core exceptions (top-level classes).
pub(super) const RUBY: Hierarchy = &[
    ("ArgumentError", "StandardError"),
    ("ClosedQueueError", "StopIteration"),
    ("EOFError", "IOError"),
    ("EncodingError", "StandardError"),
    ("FiberError", "StandardError"),
    ("FloatDomainError", "RangeError"),
    ("FrozenError", "RuntimeError"),
    ("IOError", "StandardError"),
    ("IndexError", "StandardError"),
    ("Interrupt", "SignalException"),
    ("KeyError", "IndexError"),
    ("LoadError", "ScriptError"),
    ("LocalJumpError", "StandardError"),
    ("NameError", "StandardError"),
    ("NoMatchingPatternError", "StandardError"),
    ("NoMatchingPatternKeyError", "NoMatchingPatternError"),
    ("NoMemoryError", "Exception"),
    ("NoMethodError", "NameError"),
    ("NotImplementedError", "ScriptError"),
    ("RangeError", "StandardError"),
    ("RegexpError", "StandardError"),
    ("RuntimeError", "StandardError"),
    ("ScriptError", "Exception"),
    ("SecurityError", "Exception"),
    ("SignalException", "Exception"),
    ("StandardError", "Exception"),
    ("StopIteration", "IndexError"),
    ("SyntaxError", "ScriptError"),
    ("SystemCallError", "StandardError"),
    ("SystemExit", "Exception"),
    ("SystemStackError", "Exception"),
    ("ThreadError", "StandardError"),
    ("TypeError", "StandardError"),
    ("UncaughtThrowError", "ArgumentError"),
    ("ZeroDivisionError", "StandardError"),
];

/// C++ standard library exceptions, without the `std::` qualifier.
pub(super) const CPP: Hierarchy = &[
    ("bad_alloc", "exception"),
    ("bad_any_cast", "bad_cast"),
    ("bad_array_new_length", "bad_alloc"),
    ("bad_cast", "exception"),
    ("bad_exception", "exception"),
    ("bad_function_call", "exception"),
    ("bad_optional_access", "exception"),
    ("bad_typeid", "exception"),
    ("bad_variant_access", "exception"),
    ("bad_weak_ptr", "exception"),
    ("domain_error", "logic_error"),
    ("filesystem_error", "system_error"),
    ("future_error", "logic_error"),
    ("invalid_argument", "logic_error"),
    ("length_error", "logic_error"),
    ("logic_error", "exception"),
    ("out_of_range", "logic_error"),
    ("overflow_error", "runtime_error"),
    ("range_error", "runtime_error"),
    ("regex_error", "runtime_error"),
    ("runtime_error", "exception"),
    ("system_error", "runtime_error"),
    ("underflow_error", "runtime_error"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    type Pairs = BTreeSet<(String, String)>;

    /// The `child parent` pairs of a fixture: every line that is not a `#` comment.
    fn fixture(text: &str) -> Pairs {
        text.lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .map(|l| {
                let fields: Vec<&str> = l.split_whitespace().collect();
                assert_eq!(fields.len(), 2, "not a `child parent` line: {l:?}");
                (fields[0].to_string(), fields[1].to_string())
            })
            .collect()
    }

    /// A table holds exactly the pairs of its fixture, each once, and is free of cycles.
    fn assert_pinned(table: Hierarchy, text: &str) {
        let listed: Pairs = table
            .iter()
            .map(|(c, p)| (c.to_string(), p.to_string()))
            .collect();
        assert_eq!(listed.len(), table.len(), "a pair is listed twice");
        let want = fixture(text);
        let missing: Vec<_> = want.difference(&listed).collect();
        let extra: Vec<_> = listed.difference(&want).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "table and fixture differ\n in the fixture only: {missing:?}\n in the table only: {extra:?}"
        );
        for (class, _) in table {
            let mut open = vec![*class];
            let mut steps = 0;
            while let Some(current) = open.pop() {
                steps += 1;
                assert!(steps <= table.len() * table.len(), "cycle through {class}");
                open.extend(table.iter().filter(|(c, _)| *c == current).map(|(_, p)| *p));
            }
        }
        // The header says where the pairs come from.
        assert!(
            text.starts_with('#') && (text.contains("# Runtime: ") || text.contains("# READ")),
            "the fixture has no header naming its source"
        );
    }

    #[test]
    fn python_table_is_its_fixture() {
        assert_pinned(
            PYTHON,
            include_str!("../../tests/fixtures/exception_hierarchy/python.txt"),
        );
    }

    #[test]
    fn java_table_is_its_fixture() {
        assert_pinned(
            JAVA,
            include_str!("../../tests/fixtures/exception_hierarchy/java.txt"),
        );
    }

    #[test]
    fn dotnet_table_is_its_fixture() {
        assert_pinned(
            DOTNET,
            include_str!("../../tests/fixtures/exception_hierarchy/dotnet.txt"),
        );
    }

    #[test]
    fn javascript_table_is_its_fixture() {
        assert_pinned(
            JS,
            include_str!("../../tests/fixtures/exception_hierarchy/javascript.txt"),
        );
    }

    #[test]
    fn php_table_is_its_fixture() {
        assert_pinned(
            PHP,
            include_str!("../../tests/fixtures/exception_hierarchy/php.txt"),
        );
    }

    #[test]
    fn ruby_table_is_its_fixture() {
        assert_pinned(
            RUBY,
            include_str!("../../tests/fixtures/exception_hierarchy/ruby.txt"),
        );
    }

    #[test]
    fn cpp_table_is_its_fixture() {
        assert_pinned(
            CPP,
            include_str!("../../tests/fixtures/exception_hierarchy/cpp.txt"),
        );
    }

    /// Control for `assert_pinned`: a table that lost a pair, or gained one, is caught.
    #[test]
    fn a_table_that_drifts_from_its_fixture_is_caught() {
        let text = "# Runtime: none\nKeyError LookupError\nLookupError Exception\n";
        assert_pinned(
            &[("KeyError", "LookupError"), ("LookupError", "Exception")],
            text,
        );
        for drifted in [
            &[("KeyError", "LookupError")][..],
            &[
                ("KeyError", "LookupError"),
                ("LookupError", "Exception"),
                ("IndexError", "LookupError"),
            ][..],
            &[("KeyError", "Exception"), ("LookupError", "Exception")][..],
        ] {
            let drifted: Hierarchy = Box::leak(drifted.to_vec().into_boxed_slice());
            assert!(
                std::panic::catch_unwind(|| assert_pinned(drifted, text)).is_err(),
                "{drifted:?}"
            );
        }
    }
}
