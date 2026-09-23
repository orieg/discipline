_discipline() {
    local i cur prev opts cmd
    COMPREPLY=()
    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
        cur="$2"
    else
        cur="${COMP_WORDS[COMP_CWORD]}"
    fi
    prev="$3"
    cmd=""
    opts=""

    for i in "${COMP_WORDS[@]:0:COMP_CWORD}"
    do
        case "${cmd},${i}" in
            ",$1")
                cmd="discipline"
                ;;
            discipline,baseline)
                cmd="discipline__subcmd__baseline"
                ;;
            discipline,bench)
                cmd="discipline__subcmd__bench"
                ;;
            discipline,check)
                cmd="discipline__subcmd__check"
                ;;
            discipline,completions)
                cmd="discipline__subcmd__completions"
                ;;
            discipline,diff)
                cmd="discipline__subcmd__diff"
                ;;
            discipline,docs)
                cmd="discipline__subcmd__docs"
                ;;
            discipline,doctor)
                cmd="discipline__subcmd__doctor"
                ;;
            discipline,explain)
                cmd="discipline__subcmd__explain"
                ;;
            discipline,gates)
                cmd="discipline__subcmd__gates"
                ;;
            discipline,help)
                cmd="discipline__subcmd__help"
                ;;
            discipline,hook)
                cmd="discipline__subcmd__hook"
                ;;
            discipline,init)
                cmd="discipline__subcmd__init"
                ;;
            discipline,install-hooks)
                cmd="discipline__subcmd__install__subcmd__hooks"
                ;;
            discipline,mcp)
                cmd="discipline__subcmd__mcp"
                ;;
            discipline,replay)
                cmd="discipline__subcmd__replay"
                ;;
            discipline,schema)
                cmd="discipline__subcmd__schema"
                ;;
            discipline,self-test)
                cmd="discipline__subcmd__self__subcmd__test"
                ;;
            discipline__subcmd__bench,derive)
                cmd="discipline__subcmd__bench__subcmd__derive"
                ;;
            discipline__subcmd__bench,help)
                cmd="discipline__subcmd__bench__subcmd__help"
                ;;
            discipline__subcmd__bench__subcmd__help,derive)
                cmd="discipline__subcmd__bench__subcmd__help__subcmd__derive"
                ;;
            discipline__subcmd__bench__subcmd__help,help)
                cmd="discipline__subcmd__bench__subcmd__help__subcmd__help"
                ;;
            discipline__subcmd__help,baseline)
                cmd="discipline__subcmd__help__subcmd__baseline"
                ;;
            discipline__subcmd__help,bench)
                cmd="discipline__subcmd__help__subcmd__bench"
                ;;
            discipline__subcmd__help,check)
                cmd="discipline__subcmd__help__subcmd__check"
                ;;
            discipline__subcmd__help,completions)
                cmd="discipline__subcmd__help__subcmd__completions"
                ;;
            discipline__subcmd__help,diff)
                cmd="discipline__subcmd__help__subcmd__diff"
                ;;
            discipline__subcmd__help,docs)
                cmd="discipline__subcmd__help__subcmd__docs"
                ;;
            discipline__subcmd__help,doctor)
                cmd="discipline__subcmd__help__subcmd__doctor"
                ;;
            discipline__subcmd__help,explain)
                cmd="discipline__subcmd__help__subcmd__explain"
                ;;
            discipline__subcmd__help,gates)
                cmd="discipline__subcmd__help__subcmd__gates"
                ;;
            discipline__subcmd__help,help)
                cmd="discipline__subcmd__help__subcmd__help"
                ;;
            discipline__subcmd__help,hook)
                cmd="discipline__subcmd__help__subcmd__hook"
                ;;
            discipline__subcmd__help,init)
                cmd="discipline__subcmd__help__subcmd__init"
                ;;
            discipline__subcmd__help,install-hooks)
                cmd="discipline__subcmd__help__subcmd__install__subcmd__hooks"
                ;;
            discipline__subcmd__help,mcp)
                cmd="discipline__subcmd__help__subcmd__mcp"
                ;;
            discipline__subcmd__help,replay)
                cmd="discipline__subcmd__help__subcmd__replay"
                ;;
            discipline__subcmd__help,schema)
                cmd="discipline__subcmd__help__subcmd__schema"
                ;;
            discipline__subcmd__help,self-test)
                cmd="discipline__subcmd__help__subcmd__self__subcmd__test"
                ;;
            discipline__subcmd__help__subcmd__bench,derive)
                cmd="discipline__subcmd__help__subcmd__bench__subcmd__derive"
                ;;
            discipline__subcmd__help__subcmd__hook,install)
                cmd="discipline__subcmd__help__subcmd__hook__subcmd__install"
                ;;
            discipline__subcmd__help__subcmd__hook,run)
                cmd="discipline__subcmd__help__subcmd__hook__subcmd__run"
                ;;
            discipline__subcmd__hook,help)
                cmd="discipline__subcmd__hook__subcmd__help"
                ;;
            discipline__subcmd__hook,install)
                cmd="discipline__subcmd__hook__subcmd__install"
                ;;
            discipline__subcmd__hook,run)
                cmd="discipline__subcmd__hook__subcmd__run"
                ;;
            discipline__subcmd__hook__subcmd__help,help)
                cmd="discipline__subcmd__hook__subcmd__help__subcmd__help"
                ;;
            discipline__subcmd__hook__subcmd__help,install)
                cmd="discipline__subcmd__hook__subcmd__help__subcmd__install"
                ;;
            discipline__subcmd__hook__subcmd__help,run)
                cmd="discipline__subcmd__hook__subcmd__help__subcmd__run"
                ;;
            *)
                ;;
        esac
    done

    case "${cmd}" in
        discipline)
            opts="-h -V --help --version check diff baseline init gates schema self-test completions docs install-hooks hook explain replay mcp bench doctor help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 1 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__baseline)
            opts="-c -b -s -h --config --config-override --enable --disable --write --baseline-file --base --suite --whole-tree --all-severities --fail-on-warnings --trust-workspace --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -c)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config-override)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --enable)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --disable)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --baseline-file)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --base)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -b)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --suite)
                    COMPREPLY=($(compgen -W "all agent-guard hygiene integrity quality verification bench" -- "${cur}"))
                    return 0
                    ;;
                -s)
                    COMPREPLY=($(compgen -W "all agent-guard hygiene integrity quality verification bench" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__bench)
            opts="-h --help derive help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__bench__subcmd__derive)
            opts="-h --baseline --allow-mixed-commits --ceiling-pct --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --baseline)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --ceiling-pct)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__bench__subcmd__help)
            opts="derive help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__bench__subcmd__help__subcmd__derive)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__bench__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__check)
            opts="-c -s -b -q -f -o -h --config --config-override --enable --disable --suite --base --commit --commit-range --staged --commit-msg-file --pr-body-file --pr-title --fail-on-warnings --fail-on-overrides --advisory --comment --policy-from --actor --directive-sources --trust-workspace --quiet --format --json-out --output-file --report-gitlab --report-junit --report-sarif --bench-provenance --allow-cross-host-bench --bench-base-file --bench-head-file --baseline-file --no-baseline --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -c)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config-override)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --enable)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --disable)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --suite)
                    COMPREPLY=($(compgen -W "all agent-guard hygiene integrity quality verification bench" -- "${cur}"))
                    return 0
                    ;;
                -s)
                    COMPREPLY=($(compgen -W "all agent-guard hygiene integrity quality verification bench" -- "${cur}"))
                    return 0
                    ;;
                --base)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -b)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --commit)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --commit-range)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --pr-body-file)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --commit-msg-file)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --pr-title)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --policy-from)
                    COMPREPLY=($(compgen -W "head base" -- "${cur}"))
                    return 0
                    ;;
                --actor)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --directive-sources)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --format)
                    COMPREPLY=($(compgen -W "terminal github-summary json junit sarif gitlab agent-prompt" -- "${cur}"))
                    return 0
                    ;;
                -f)
                    COMPREPLY=($(compgen -W "terminal github-summary json junit sarif gitlab agent-prompt" -- "${cur}"))
                    return 0
                    ;;
                --json-out)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output-file)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -o)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --report-gitlab)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --report-junit)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --report-sarif)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --bench-provenance)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --bench-base-file)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --bench-head-file)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --baseline-file)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__completions)
            opts="-h --help bash elvish fish powershell zsh"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__diff)
            opts="-c -s -b -f -o -h --config --config-override --enable --disable --suite --base --format --json-out --output-file --report-gitlab --report-junit --report-sarif --baseline-file --no-baseline --trust-workspace --advisory --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -c)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config-override)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --enable)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --disable)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --suite)
                    COMPREPLY=($(compgen -W "all agent-guard hygiene integrity quality verification bench" -- "${cur}"))
                    return 0
                    ;;
                -s)
                    COMPREPLY=($(compgen -W "all agent-guard hygiene integrity quality verification bench" -- "${cur}"))
                    return 0
                    ;;
                --base)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -b)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --format)
                    COMPREPLY=($(compgen -W "terminal github-summary json junit sarif gitlab agent-prompt" -- "${cur}"))
                    return 0
                    ;;
                -f)
                    COMPREPLY=($(compgen -W "terminal github-summary json junit sarif gitlab agent-prompt" -- "${cur}"))
                    return 0
                    ;;
                --json-out)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --output-file)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -o)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --report-gitlab)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --report-junit)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --report-sarif)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --baseline-file)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__docs)
            opts="-h --check --write --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__doctor)
            opts="-f -h --branch --repo --local-only --strict --format --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --branch)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --repo)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --format)
                    COMPREPLY=($(compgen -W "text json" -- "${cur}"))
                    return 0
                    ;;
                -f)
                    COMPREPLY=($(compgen -W "text json" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__explain)
            opts="-c -h --config --config-override --enable --disable --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -c)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config-override)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --enable)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --disable)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__gates)
            opts="-c -h --config --config-override --enable --disable --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -c)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config-override)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --enable)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --disable)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__help)
            opts="check diff baseline init gates schema self-test completions docs install-hooks hook explain replay mcp bench doctor help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__help__subcmd__baseline)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__help__subcmd__bench)
            opts="derive"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__help__subcmd__bench__subcmd__derive)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__help__subcmd__check)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__help__subcmd__completions)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__help__subcmd__diff)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__help__subcmd__docs)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__help__subcmd__doctor)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__help__subcmd__explain)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__help__subcmd__gates)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__help__subcmd__hook)
            opts="run install"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__help__subcmd__hook__subcmd__install)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__help__subcmd__hook__subcmd__run)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__help__subcmd__init)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__help__subcmd__install__subcmd__hooks)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__help__subcmd__mcp)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__help__subcmd__replay)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__help__subcmd__schema)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__help__subcmd__self__subcmd__test)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__hook)
            opts="-h --help run install help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__hook__subcmd__help)
            opts="run install help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__hook__subcmd__help__subcmd__help)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__hook__subcmd__help__subcmd__install)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__hook__subcmd__help__subcmd__run)
            opts=""
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 4 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__hook__subcmd__install)
            opts="-h --agent --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --agent)
                    COMPREPLY=($(compgen -W "claude-code codex cursor aider" -- "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__hook__subcmd__run)
            opts="-b -h --agent --base --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 3 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --agent)
                    COMPREPLY=($(compgen -W "claude-code codex cursor aider" -- "${cur}"))
                    return 0
                    ;;
                --base)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -b)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__init)
            opts="-n -h --name --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --name)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -n)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__install__subcmd__hooks)
            opts="-f -h --force --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__mcp)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__replay)
            opts="-c -h --last --ref --config --json --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --last)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --ref)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                --config)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                -c)
                    COMPREPLY=($(compgen -f "${cur}"))
                    return 0
                    ;;
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__schema)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
        discipline__subcmd__self__subcmd__test)
            opts="-h --help"
            if [[ ${cur} == -* || ${COMP_CWORD} -eq 2 ]] ; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                *)
                    COMPREPLY=()
                    ;;
            esac
            COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
            return 0
            ;;
    esac
}

if [[ "${BASH_VERSINFO[0]}" -eq 4 && "${BASH_VERSINFO[1]}" -ge 4 || "${BASH_VERSINFO[0]}" -gt 4 ]]; then
    complete -F _discipline -o nosort -o bashdefault -o default discipline
else
    complete -F _discipline -o bashdefault -o default discipline
fi
