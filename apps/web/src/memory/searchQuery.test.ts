import { describe, expect, it } from 'vitest'

import { describeFilter, parseSearchQuery, queryFilters, withToken, withoutToken, type ParsedQuery } from './searchQuery'

const cases = Object.values(
  import.meta.glob<{ default: { query: string; parsed: Partial<ParsedQuery> }[] }>('../../../server/src/search/query_parity.json', {
    eager: true,
  }),
)[0].default

const EMPTY: ParsedQuery = {
  text: '',
  from: [],
  inChannels: [],
  inPeople: [],
  after: null,
  before: null,
  has: [],
  is: [],
  kinds: [],
  errors: [],
}

describe('the search grammar', () => {
  it('reads every parity case as the server does', () => {
    expect(cases.length).toBeGreaterThan(10)
    for (const { query, parsed } of cases) {
      expect(parseSearchQuery(query), query).toEqual({ ...EMPTY, ...parsed })
    }
  })

  it('lists filters for chips and takes them out again', () => {
    const query = 'launch from:@lena has:banana "in:#x"'
    const filters = queryFilters(query)
    expect(filters.map((filter) => [filter.token, filter.ok])).toEqual([
      ['from:@lena', true],
      ['has:banana', false],
    ])
    expect(describeFilter(filters[0])).toBe('From @lena')
    expect(withoutToken(query, 'has:banana')).toBe('launch from:@lena "in:#x"')
    expect(withToken('launch', 'is:unread')).toBe('launch is:unread')
    expect(withToken('launch IS:UNREAD', 'is:unread')).toBe('launch IS:UNREAD')
  })
})
