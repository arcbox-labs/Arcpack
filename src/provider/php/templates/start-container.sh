#!/bin/sh
set -e

# Laravel 迁移（可通过 ARCPACK_SKIP_MIGRATIONS 禁用）
if [ -f /app/artisan ] && [ "${ARCPACK_SKIP_MIGRATIONS}" != "true" ]; then
    php artisan migrate --force 2>/dev/null || true
fi

# 启动 FrankenPHP
exec php-server
