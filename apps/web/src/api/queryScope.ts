let queryScopeGeneration = 0

export function currentQueryScopeGeneration() {
  return queryScopeGeneration
}

export function resetQueryScope() {
  queryScopeGeneration += 1
}
