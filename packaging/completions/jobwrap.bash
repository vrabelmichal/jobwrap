_jobwrap() {
    local cur prev
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"

    local subcommands="wrap list show logs attach signal stop open daemon config auth token launch attach-launch inspect terminals help"

    COMPREPLY=()
    if [[ "${COMP_WORDS[1]}" == signal && "$COMP_CWORD" == 3 ]]; then
        COMPREPLY=( $(compgen -W "int term hup quit stop cont kill" -- "$cur") )
        return
    fi

    case "$prev" in
        show|logs|attach|signal|stop|open)
            # Do not start the daemon or expose job output when the user presses Tab.
            return
            ;;
        daemon)
            COMPREPLY=( $(compgen -W "status start stop restart" -- "$cur") )
            return
            ;;
        config)
            COMPREPLY=( $(compgen -W "path init validate effective explain" -- "$cur") )
            return
            ;;
        auth)
            COMPREPLY=( $(compgen -W "set-password remove-password status" -- "$cur") )
            return
            ;;
        token)
            COMPREPLY=( $(compgen -W "create list revoke" -- "$cur") )
            return
            ;;
    esac

    if [[ "${COMP_WORDS[1]}" == daemon && "${COMP_WORDS[2]}" == start && "$COMP_CWORD" == 3 ]]; then
        COMPREPLY=( $(compgen -W "--foreground --help" -- "$cur") )
        return
    fi

    if [[ "$cur" == -* ]]; then
        COMPREPLY=( $(compgen -W "--name --profile --detach --no-web --no-record --help --version" -- "$cur") )
    else
        COMPREPLY=( $(compgen -W "$subcommands" -- "$cur") )
        COMPREPLY+=( $(compgen -c -- "$cur") )
    fi
}

complete -o default -F _jobwrap jobwrap
