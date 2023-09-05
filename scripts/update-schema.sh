#!/usr/bin/env bash

diesel \
  --database-url 'postgres://pictrs:1234@localhost:5432/pictrs' \
  print-schema \
  --custom-type-derives "diesel::query_builder::QueryId" \
  > src/repo/postgres/schema.rs
