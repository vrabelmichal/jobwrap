_jobwrap() {
    local cur prev
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"

    local subcommands="run list show status logs attach signal stop open daemon config auth token launch attach-launch inspect terminals help"

    COMPREPLY=()
    if [[ "${COMP_WORDS[1]}" == signal && "$COMP_CWORD" == 3 ]]; then
        COMPREPLY=( $(compgen -W "int term hup quit stop cont kill" -- "$cur" || true) )
        return
    fi

    case "$prev" in
        show|status|logs|attach|signal|stop|open)
            # Do not start the daemon or expose job output when the user presses Tab.
            return
            ;;
        daemon)
            COMPREPLY=( $(compgen -W "status start stop restart" -- "$cur" || true) )
            return
            ;;
        config)
            COMPREPLY=( $(compgen -W "path init validate effective explain" -- "$cur" || true) )
            return
            ;;
        auth)
            COMPREPLY=( $(compgen -W "set-password remove-password status" -- "$cur" || true) )
            return
            ;;
        token)
            COMPREPLY=( $(compgen -W "create list revoke" -- "$cur" || true) )
            return
            ;;
    esac

    if [[ "${COMP_WORDS[1]}" == daemon && "${COMP_WORDS[2]}" == start && "$COMP_CWORD" == 3 ]]; then
        COMPREPLY=( $(compgen -W "--foreground --help" -- "$cur" || true) )
        return
    fi

    if [[ "$COMP_CWORD" == 1 ]]; then
        COMPREPLY=( $(compgen -W "$subcommands --help --version" -- "$cur" || true) )
    elif [[ "${COMP_WORDS[1]}" == run ]]; then
        local i
        for (( i=2; i<COMP_CWORD; i++ )); do
            if [[ "${COMP_WORDS[i]}" == -- ]]; then
                if (( COMP_CWORD == i + 1 )); then
                    COMPREPLY=( $(compgen -c -- "$cur" || true) )
                fi
                return
            fi
        done
        if [[ "$cur" == -* ]]; then
            COMPREPLY=( $(compgen -W "-d --detach -n --name --profile --no-web --no-record --help" -- "$cur" || true) )
        elif [[ "$prev" == run ]]; then
            COMPREPLY=( $(compgen -c -- "$cur" || true) )
        fi
    fi
}

complete -o default -F _jobwrap jobwrap
