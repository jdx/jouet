const { shout } = require('../src/fmt')

test('shout', () => expect(shout('hi')).toBe('hi!'))
