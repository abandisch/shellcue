#!/usr/bin/env bash

mkdir -p "${HOME}/.local/bin" > /dev/null 2>&1

# add ~/.local/bin to PATH
if [[ -d "${HOME}/.local/bin" ]] && [[ ":$PATH:" != *":${HOME}/.local/bin:"* ]]; then
  PATH="${PATH:+"$PATH:"}${HOME}/.local/bin"
fi

if [[ -n "${SHELLCUE_NEW_SESSION:-}" ]]; then
  unset SHELLCUE_TERM_SESSION_ID
  unset SHELLCUE_TERM
  unset SHELLCUE_NEW_SESSION
fi

# 0 = Yes, 1 = No, 2 = Fallback to SHELLCUE_TERM
if [ -z "${SHOULD_SHELLCUE_TERM_LAUNCH:-}" ]; then
  {{CLI_BINARY_NAME}} _ should-figterm-launch 1>/dev/null 2>&1
  SHOULD_SHELLCUE_TERM_LAUNCH=$?
fi

# Only launch shellcue-term if current session is not already inside PTY and command exists.
# PWSH var is set when launched by `pwsh -Login`, in which case we don't want to init.
if   [[ -t 1 ]] \
  && [[ -z "${PROCESS_LAUNCHED_BY_SHELLCUE:-}" ]] \
  && command -v {{PTY_BINARY_NAME}} 1>/dev/null 2>&1 \
  && [[ ("${SHOULD_SHELLCUE_TERM_LAUNCH}" -eq 0) || (("${SHOULD_SHELLCUE_TERM_LAUNCH}" -eq 2) && (-z "${SHELLCUE_TERM:-}" || (-z "${SHELLCUE_TERM_TMUX:-}" && -n "${TMUX:-}"))) ]]
then
  # Pty module sets SHELLCUE_TERM or SHELLCUE_TERM_TMUX to avoid running twice.
  if [ -z "${SHELLCUE_SHELL:-}" ]; then
    SHELLCUE_SHELL=$({{CLI_BINARY_NAME}} _ get-shell)
  fi
  SHELLCUE_IS_LOGIN_SHELL="${SHELLCUE_IS_LOGIN_SHELL:='0'}"

  # shellcheck disable=SC2030
  if [[ -n "${ZSH_NAME:-}" && -o login ]]; then
    SHELLCUE_IS_LOGIN_SHELL=1
  fi

  # Do not launch shellcue-term in non-interactive shells (like VSCode Tasks)
  if [[ $- == *i* ]]; then
    SHELLCUE_TERM_NAME="$(basename "${SHELLCUE_SHELL}") ({{PTY_BINARY_NAME}})"
    if [[ -z "${SHELLCUE_TERM_PATH:-}" ]]; then
      SHELLCUE_TERM_PATH="$(command -v {{PTY_BINARY_NAME}} || echo "${HOME}/.local/bin/{{PTY_BINARY_NAME}}")"
    fi

    SHELLCUE_EXECUTION_STRING="${ZSH_EXECUTION_STRING}"

    # Get initial text.
    INITIAL_TEXT=""
    # shellcheck disable=SC2031
    while read -rt 0; do
      INITIAL_TEXT="${INITIAL_TEXT}${REPLY}\n"
    done
    SHELLCUE_EXECUTION_STRING="${SHELLCUE_EXECUTION_STRING}" SHELLCUE_START_TEXT="$(printf "%b" "${INITIAL_TEXT}")" SHELLCUE_SHELL="${SHELLCUE_SHELL}" SHELLCUE_IS_LOGIN_SHELL="${SHELLCUE_IS_LOGIN_SHELL}" exec -a "${SHELLCUE_TERM_NAME}" "${SHELLCUE_TERM_PATH}"
  fi
fi
