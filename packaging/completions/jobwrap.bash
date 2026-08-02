_jobwrap() {
    local cur prev
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"

    local subcommands="list show logs attach signal stop open daemon config auth token version help"

    case "$prev" in
        show|logs|attach|signal|stop|open)
            local jobs=$(jobwrap list 2>/dev/null | awk 'NR>1 {print $1}')
            COMPREPLY=( $(compgen -W "$jobs" -- "$cur") )
            return
            ;;
        signal)
            COMPREPLY=( $(compgen -W "int term hup quit stop cont kill" -- "$cur") )
            return
            ;;
        daemon)
            COMPREPLY=( $(compgen -W "status start stop" -- "$cur") )
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

    if [[ "$cur" == -* ]]; then
        COMPREPLY=( $(compgen -W "--name --profile --detach --no-web --no-record --help --version" -- "$cur") )
    else
        COMPREPLY=( $(compgen -W "$subcommands" -- "$cur") )
        COMPREPLY+=( $(compgen -c -- "$cur") )
    fi
}

complete -F _jobwrap jobwrap
