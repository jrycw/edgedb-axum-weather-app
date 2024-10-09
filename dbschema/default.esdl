  module default {
    # scalar
    scalar type Latitude extending float64 {
      constraint max_value(90.0);
      constraint min_value(-90.0);
    }
    scalar type Longitude extending float64 {
      constraint max_value(180.0);
      constraint min_value(-180.0);
    }
    scalar type Temperature extending float64 {
      constraint max_value(70.0);
      constraint min_value(-100.0);
    }
    
    # object type
    type Conditions {
        required city: City {
            on target delete delete source;
        }
        required temperature: Temperature;
        required time: str;
        constraint exclusive on ((.time, .city));
    }


    type City {
      required name: str;
      required latitude: Latitude;
      required longitude: Longitude;
      multi conditions := (select .<city[is Conditions] order by .time);
      key := .name ++ <str><int64>.latitude ++ <str><int64>.longitude;
      constraint exclusive on (.key);
    }
  }